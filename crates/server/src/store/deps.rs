//! Dependencies between issues. A dependency is a `blocks` relation: "A
//! depends on B" is stored as "B blocks A". Depth is the longest chain of
//! open blockers.
//!
//! Every issue read needs `depth` and `has_open_dependents`, and both can
//! depend on a chain through the whole database. So the server reads every
//! `blocks` edge once, computes both values for every issue in Rust, and
//! keeps the result in memory (LLM-216). Each computation visits each issue
//! and edge once. The table `dep_graph_token` holds a random token that
//! triggers replace on every write that can change the graph, so a read
//! rebuilds the graph only after such a write. Before, each page of issues
//! walked its chains again with recursive queries. On one chain of 49,000
//! issues, that took half a second per page.
//!
//! The recursive queries below serve the single-issue filters and the tree.
//! They use CROSS JOIN, which makes SQLite keep the written join order. Left
//! to itself, its planner starts from `statuses` and walks every issue for
//! each step of the recursion. The walk up needs the index from migration
//! 0007 for the same reason: without it, each step scans every `blocks` row.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use llmtodo_api_types::{DepNode, IssueKey, IssueLink};
use sqlx::SqliteConnection;

use crate::error::{ApiError, ApiResult};
use crate::store::{issues, teams};

/// How deep `issue deps` goes. A node takes two levels of JSON nesting, and
/// serde refuses to read more than 128.
const MAX_TREE_DEPTH: usize = 40;

/// The dependency graph of every live issue, with the values reads need.
/// Only live issues on at least one `blocks` edge appear in it. Each issue
/// has an index, and the lists below are by index.
#[derive(Default)]
struct Graph {
    token: Vec<u8>,
    index: HashMap<String, u32>,
    ids: Vec<String>,
    /// The open blockers of each open issue, in key order.
    blockers: Vec<Vec<u32>>,
    depth: Vec<u32>,
    /// True for a closed issue that an open issue still waits for, directly
    /// or through other closed issues.
    held: Vec<bool>,
}

/// The last graph built. One entry is enough: a server has one database.
/// Tests run several servers in one process, and their random tokens never
/// match, so a mix-up costs only a rebuild.
static GRAPH: Mutex<Option<Arc<Graph>>> = Mutex::new(None);

/// Separates the fields in the strings that the graph queries return.
const SEP: char = '\u{1f}';

/// Up to this many issues on edges, the graph looks its issues up by ID.
/// Above it, it scans every live issue.
const SMALL_GRAPH: usize = 5000;

/// A live issue on a `blocks` edge.
struct Node<'a> {
    id: &'a str,
    team_id: &'a str,
    number: i64,
    open: bool,
}

impl Graph {
    /// The current graph, built again when a write changed it since.
    async fn current(conn: &mut SqliteConnection) -> ApiResult<Arc<Self>> {
        let token: Vec<u8> = sqlx::query_scalar("SELECT token FROM dep_graph_token")
            .fetch_one(&mut *conn)
            .await?;
        if let Some(g) = GRAPH.lock().expect("graph cache lock").as_ref()
            && g.token == token
        {
            return Ok(Arc::clone(g));
        }
        // Each query returns one string. sqlx hands over rows one at a time,
        // and 100,000 rows took 100 ms that way, against 15 ms as one string.
        let edges: Option<String> = sqlx::query_scalar(
            "SELECT group_concat(from_id || char(31) || to_id, char(31))
             FROM relations WHERE type = 'blocks'",
        )
        .fetch_one(&mut *conn)
        .await?;
        let edges = edges.unwrap_or_default();
        let on_edges: HashSet<&str> = edges.split(SEP).filter(|f| !f.is_empty()).collect();
        // Looking issues up by ID costs about 3 µs each, and a scan of every
        // live issue about 0.4 µs each. So a small graph looks its issues
        // up, and a large one scans.
        let node_fields = "group_concat(i.id || char(31) || i.team_id || char(31) || i.number
            || char(31) || (s.type NOT IN ('completed', 'canceled')), char(31))";
        let nodes: Option<String> = if on_edges.is_empty() {
            None
        } else if on_edges.len() <= SMALL_GRAPH {
            let ids: Vec<&str> = on_edges.iter().copied().collect();
            sqlx::query_scalar(crate::sql!(
                "SELECT {node_fields} FROM json_each(?) j
                 CROSS JOIN issues i ON i.id = j.value
                 CROSS JOIN statuses s ON s.id = i.status_id
                 WHERE i.deleted_at IS NULL"
            ))
            .bind(serde_json::to_string(&ids)?)
            .fetch_one(&mut *conn)
            .await?
        } else {
            sqlx::query_scalar(crate::sql!(
                "SELECT {node_fields} FROM issues i
                 CROSS JOIN statuses s ON s.id = i.status_id
                 WHERE i.deleted_at IS NULL"
            ))
            .fetch_one(&mut *conn)
            .await?
        };
        let graph = Arc::new(Self::build(
            token,
            &edges,
            nodes.as_deref().unwrap_or(""),
            &on_edges,
        )?);
        *GRAPH.lock().expect("graph cache lock") = Some(Arc::clone(&graph));
        Ok(graph)
    }

    /// Builds the graph from the two query strings. `edges` holds blocker
    /// and issue IDs in turn. `nodes` holds ID, team, number and an open
    /// flag for live issues, and those not in `on_edges` are left out. An
    /// edge to a trashed issue is left out.
    fn build(
        token: Vec<u8>,
        edges: &str,
        nodes: &str,
        on_edges: &HashSet<&str>,
    ) -> ApiResult<Self> {
        let bad = || ApiError::internal("bad dependency graph row");
        let mut list: Vec<Node<'_>> = Vec::new();
        let mut fields = nodes.split(SEP).filter(|f| !f.is_empty());
        while let Some(id) = fields.next() {
            let (Some(team_id), Some(number), Some(open)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(bad());
            };
            if !on_edges.contains(id) {
                continue;
            }
            list.push(Node {
                id,
                team_id,
                number: number.parse().map_err(|_| bad())?,
                open: open == "1",
            });
        }
        // Indexes follow key order, so sorting blockers by index sorts them
        // by key.
        list.sort_unstable_by(|a, b| (a.team_id, a.number).cmp(&(b.team_id, b.number)));
        let index: HashMap<String, u32> = list
            .iter()
            .zip(0u32..)
            .map(|(n, i)| (n.id.to_string(), i))
            .collect();
        let open: Vec<bool> = list.iter().map(|n| n.open).collect();
        let mut blockers: Vec<Vec<u32>> = vec![Vec::new(); list.len()];
        let mut held_edges: Vec<(u32, u32)> = Vec::new();
        let mut fields = edges.split(SEP).filter(|f| !f.is_empty());
        while let Some(from) = fields.next() {
            let to = fields.next().ok_or_else(bad)?;
            let (Some(&b), Some(&d)) = (index.get(from), index.get(to)) else {
                continue;
            };
            if !open[b as usize] {
                held_edges.push((b, d));
            } else if open[d as usize] {
                blockers[d as usize].push(b);
            }
        }
        for list in &mut blockers {
            list.sort_unstable();
        }
        Ok(Self {
            token,
            depth: chain_depths(&blockers),
            held: held_by(&held_edges, &open),
            ids: list.iter().map(|n| n.id.to_string()).collect(),
            index,
            blockers,
        })
    }

    fn of(&self, id: &str) -> Option<usize> {
        self.index.get(id).map(|&i| i as usize)
    }
}

/// The open blockers of a set of issues, with the whole graph behind them.
pub struct Blockers {
    graph: Arc<Graph>,
    /// The direct open blockers of the issues asked for.
    links: HashMap<String, IssueLink>,
}

impl Blockers {
    /// Loads the graph, and the short forms of the direct open blockers of
    /// `ids`. A closed or trashed issue has no open blockers.
    pub async fn load(
        conn: &mut SqliteConnection,
        ids: &[String],
        statuses: &HashMap<String, teams::StatusRow>,
    ) -> ApiResult<Self> {
        let graph = Graph::current(conn).await?;
        let mut direct: Vec<u32> = ids
            .iter()
            .filter_map(|id| graph.of(id))
            .flat_map(|i| graph.blockers[i].iter().copied())
            .collect();
        direct.sort_unstable();
        direct.dedup();
        let direct: Vec<String> = direct
            .into_iter()
            .map(|b| graph.ids[b as usize].clone())
            .collect();
        let links = issues::link_map_with(conn, &direct, statuses).await?;
        Ok(Self { graph, links })
    }

    pub fn depth(&self, id: &str) -> u32 {
        self.graph.of(id).map_or(0, |i| self.graph.depth[i])
    }

    /// True when `id` is closed and an open issue still waits for it.
    pub fn has_open_dependents(&self, id: &str) -> bool {
        self.graph.of(id).is_some_and(|i| self.graph.held[i])
    }

    pub fn open_blockers(&self, id: &str) -> Vec<IssueLink> {
        self.graph.of(id).map_or_else(Vec::new, |i| {
            self.graph.blockers[i]
                .iter()
                .filter_map(|&b| self.links.get(&self.graph.ids[b as usize]).cloned())
                .collect()
        })
    }

    /// The keys along the longest chain, from the nearest blocker to the
    /// farthest. Between equal chains, the lowest key wins.
    pub async fn longest_chain(
        &self,
        conn: &mut SqliteConnection,
        id: &str,
        statuses: &HashMap<String, teams::StatusRow>,
    ) -> ApiResult<Vec<IssueKey>> {
        let g = &self.graph;
        let mut chain: Vec<String> = Vec::new();
        if let Some(start) = g.of(id) {
            let mut seen = vec![false; g.ids.len()];
            seen[start] = true;
            let mut current = start;
            // `max_by_key` keeps the last of equal items, so walk in reverse.
            while let Some(&next) = g.blockers[current]
                .iter()
                .rev()
                .max_by_key(|&&b| g.depth[b as usize])
            {
                let next = next as usize;
                if seen[next] {
                    break;
                }
                seen[next] = true;
                chain.push(g.ids[next].clone());
                current = next;
            }
        }
        Ok(issues::links(conn, &chain, statuses)
            .await?
            .into_iter()
            .map(|l| l.key)
            .collect())
    }
}

/// The longest path below each node, from the blockers of each node. It
/// runs on an explicit stack, so a very long chain cannot overflow the
/// thread's stack. An edge back onto the current path is ignored, so a cycle
/// in old data cannot hang it. Each node and edge is visited once.
fn chain_depths(blockers: &[Vec<u32>]) -> Vec<u32> {
    let mut depth: Vec<Option<u32>> = vec![None; blockers.len()];
    let mut on_path = vec![false; blockers.len()];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for root in 0..blockers.len() {
        if depth[root].is_some() {
            continue;
        }
        on_path[root] = true;
        stack.push((root, 0));
        while let Some((node, next)) = stack.last_mut() {
            let node = *node;
            if let Some(&blocker) = blockers[node].get(*next) {
                *next += 1;
                let blocker = blocker as usize;
                if depth[blocker].is_none() && !on_path[blocker] {
                    on_path[blocker] = true;
                    stack.push((blocker, 0));
                }
                continue;
            }
            let d = blockers[node]
                .iter()
                .filter_map(|&b| depth[b as usize])
                .map(|d| d + 1)
                .max()
                .unwrap_or(0);
            depth[node] = Some(d);
            on_path[node] = false;
            stack.pop();
        }
    }
    depth.into_iter().map(|d| d.unwrap_or(0)).collect()
}

/// The open, live issues that wait for `start`, directly or through a chain.
/// `start` itself may be closed or trashed. A closed issue ends the chain.
pub async fn dependents(conn: &mut SqliteConnection, start: &str) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE down(id) AS (
            SELECT ?
            UNION
            SELECT d.id FROM down
            CROSS JOIN relations r ON r.from_id = down.id AND r.type = 'blocks'
            CROSS JOIN issues d ON d.id = r.to_id
            CROSS JOIN statuses s ON s.id = d.status_id
            WHERE d.deleted_at IS NULL AND s.type NOT IN ('completed', 'canceled')
         )
         SELECT id FROM down WHERE id <> ?",
    )
    .bind(start)
    .bind(start)
    .fetch_all(&mut *conn)
    .await?)
}

/// The closed issues held by an open dependent, from (closed blocker,
/// dependent) edges and the open flag of each node. A closed blocker of a
/// held issue is held too.
fn held_by(edges: &[(u32, u32)], open: &[bool]) -> Vec<bool> {
    let mut up: Vec<Vec<u32>> = vec![Vec::new(); open.len()];
    let mut held = vec![false; open.len()];
    let mut queue: VecDeque<u32> = VecDeque::new();
    for &(blocker, dependent) in edges {
        if open[dependent as usize] {
            if !held[blocker as usize] {
                held[blocker as usize] = true;
                queue.push_back(blocker);
            }
        } else {
            up[dependent as usize].push(blocker);
        }
    }
    while let Some(node) = queue.pop_front() {
        for &b in &up[node as usize] {
            if !held[b as usize] {
                held[b as usize] = true;
                queue.push_back(b);
            }
        }
    }
    held
}

/// The closed, live issues that `start` waits for, directly or through
/// other closed issues. `start` itself is left out.
pub async fn closed_blockers(conn: &mut SqliteConnection, start: &str) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE up(id) AS (
            SELECT ?
            UNION
            SELECT b.id FROM up
            CROSS JOIN relations r ON r.type = 'blocks' AND r.to_id = up.id
            CROSS JOIN issues b ON b.id = r.from_id
            CROSS JOIN statuses s ON s.id = b.status_id
            WHERE b.deleted_at IS NULL AND s.type IN ('completed', 'canceled')
         )
         SELECT id FROM up WHERE id <> ?",
    )
    .bind(start)
    .bind(start)
    .fetch_all(&mut *conn)
    .await?)
}

/// The keys on a shortest `blocks` path from `start` to `target`, both ends
/// included. Closed and trashed issues count: reopening one must not be able
/// to complete a cycle.
pub async fn blocks_path(
    conn: &mut SqliteConnection,
    start: &str,
    target: &str,
) -> ApiResult<Option<Vec<String>>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "WITH RECURSIVE reach(id) AS (
            SELECT ?
            UNION
            SELECT r.to_id FROM reach CROSS JOIN relations r ON r.from_id = reach.id AND r.type = 'blocks'
         )
         SELECT r.from_id, r.to_id FROM reach
         CROSS JOIN relations r ON r.from_id = reach.id AND r.type = 'blocks'",
    )
    .bind(start)
    .fetch_all(&mut *conn)
    .await?;
    let mut next: HashMap<&str, Vec<&str>> = HashMap::new();
    for (from, to) in &rows {
        next.entry(from).or_default().push(to);
    }
    let Some(path) = shortest_path(&next, start, target) else {
        return Ok(None);
    };
    let ids: Vec<String> = path.iter().map(ToString::to_string).collect();
    let keys: HashMap<String, String> =
        sqlx::query_as("SELECT id, key FROM issues WHERE id IN (SELECT value FROM json_each(?))")
            .bind(serde_json::to_string(&ids)?)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
    Ok(Some(
        ids.iter().filter_map(|id| keys.get(id).cloned()).collect(),
    ))
}

fn shortest_path<'a>(
    next: &HashMap<&'a str, Vec<&'a str>>,
    start: &'a str,
    target: &'a str,
) -> Option<Vec<&'a str>> {
    let mut came_from: HashMap<&str, &str> = HashMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        if node == target {
            let mut path = vec![node];
            let mut at = node;
            while let Some(prev) = came_from.get(at) {
                path.push(prev);
                at = prev;
            }
            path.reverse();
            return Some(path);
        }
        for n in next.get(node).into_iter().flatten() {
            if *n != start && !came_from.contains_key(n) {
                came_from.insert(n, node);
                queue.push_back(n);
            }
        }
    }
    None
}

#[derive(Clone, Copy)]
enum Direction {
    DependsOn,
    Blocks,
}

/// Every edge reachable from `root` in one direction, as (issue, neighbor).
/// Neighbors come in key order. Trashed issues are left out, and closed ones
/// too with `open_only`.
async fn walk(
    conn: &mut SqliteConnection,
    root: &str,
    direction: Direction,
    open_only: bool,
) -> ApiResult<Vec<(String, String)>> {
    let (near, far) = match direction {
        Direction::DependsOn => ("to_id", "from_id"),
        Direction::Blocks => ("from_id", "to_id"),
    };
    let filter = "n.deleted_at IS NULL AND (? = 0 OR s.type NOT IN ('completed', 'canceled'))";
    let join = format!(
        "CROSS JOIN relations r ON r.{near} = walk.id AND r.type = 'blocks'
         CROSS JOIN issues n ON n.id = r.{far}
         CROSS JOIN statuses s ON s.id = n.status_id"
    );
    Ok(sqlx::query_as(crate::sql!(
        "WITH RECURSIVE walk(id) AS (
            SELECT ?
            UNION
            SELECT n.id FROM walk {join} WHERE {filter}
         )
         SELECT walk.id, n.id FROM walk {join} WHERE {filter}
         ORDER BY n.team_id, n.number"
    ))
    .bind(root)
    .bind(open_only)
    .bind(open_only)
    .fetch_all(&mut *conn)
    .await?)
}

/// The dependency tree around `root`, in both directions.
pub async fn tree(conn: &mut SqliteConnection, root: &str, open_only: bool) -> ApiResult<DepNode> {
    let up = walk(conn, root, Direction::DependsOn, open_only).await?;
    let down = walk(conn, root, Direction::Blocks, open_only).await?;
    let mut ids: Vec<String> = vec![root.to_string()];
    let mut seen: HashSet<&str> = HashSet::from([root]);
    for (a, b) in up.iter().chain(&down) {
        for id in [a, b] {
            if seen.insert(id) {
                ids.push(id.clone());
            }
        }
    }
    let statuses = teams::status_map(conn).await?;
    let links = issues::link_map_with(conn, &ids, &statuses).await?;
    let blockers = Blockers::load(conn, &ids, &statuses).await?;
    let builder = TreeBuilder {
        links: &links,
        blockers: &blockers,
    };
    let mut node = builder.node(root)?;
    node.depends_on = builder.children(
        root,
        &adjacency(&up),
        Direction::DependsOn,
        &mut HashSet::from([root.to_string()]),
        1,
    )?;
    node.blocks = builder.children(
        root,
        &adjacency(&down),
        Direction::Blocks,
        &mut HashSet::from([root.to_string()]),
        1,
    )?;
    Ok(node)
}

fn adjacency(edges: &[(String, String)]) -> HashMap<&str, Vec<&str>> {
    let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
    for (a, b) in edges {
        map.entry(a).or_default().push(b);
    }
    map
}

struct TreeBuilder<'a> {
    links: &'a HashMap<String, IssueLink>,
    blockers: &'a Blockers,
}

impl TreeBuilder<'_> {
    fn node(&self, id: &str) -> ApiResult<DepNode> {
        let issue = self
            .links
            .get(id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("issue", id))?;
        Ok(DepNode {
            issue,
            depth: self.blockers.depth(id),
            depends_on: Vec::new(),
            blocks: Vec::new(),
            truncated: false,
        })
    }

    /// Each issue expands once per direction. A later copy is marked
    /// truncated, so a graph where chains share blockers stays small.
    fn children(
        &self,
        id: &str,
        next: &HashMap<&str, Vec<&str>>,
        direction: Direction,
        seen: &mut HashSet<String>,
        level: usize,
    ) -> ApiResult<Vec<DepNode>> {
        let mut out = Vec::new();
        for child in next.get(id).into_iter().flatten() {
            let mut node = self.node(child)?;
            let has_more = next.get(child).is_some_and(|c| !c.is_empty());
            if seen.insert(child.to_string()) && level < MAX_TREE_DEPTH {
                let below = self.children(child, next, direction, seen, level + 1)?;
                match direction {
                    Direction::DependsOn => node.depends_on = below,
                    Direction::Blocks => node.blocks = below,
                }
            } else {
                node.truncated = has_more;
            }
            out.push(node);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blockers by index, from (issue, blockers) pairs over `n` nodes.
    fn blockers(n: usize, list: &[(u32, &[u32])]) -> Vec<Vec<u32>> {
        let mut out = vec![Vec::new(); n];
        for (a, b) in list {
            out[*a as usize] = b.to_vec();
        }
        out
    }

    #[test]
    fn depth_is_the_longest_chain() {
        let d = chain_depths(&blockers(3, &[(0, &[1]), (1, &[2])]));
        assert_eq!(d, vec![2, 1, 0]);
    }

    #[test]
    fn a_shared_blocker_counts_by_the_longest_way() {
        // 0 waits for 1 and 2, and 1 also waits for 2.
        let d = chain_depths(&blockers(3, &[(0, &[2, 1]), (1, &[2])]));
        assert_eq!(d[0], 2);
    }

    #[test]
    fn a_cycle_in_old_data_ends() {
        let d = chain_depths(&blockers(2, &[(0, &[1]), (1, &[0])]));
        assert!(d[0] <= 1 && d[1] <= 1);
    }

    #[test]
    fn a_very_long_chain_does_not_overflow() {
        let n = 200_000u32;
        let b: Vec<Vec<u32>> = (0..n)
            .map(|i| if i + 1 < n { vec![i + 1] } else { Vec::new() })
            .collect();
        assert_eq!(chain_depths(&b)[0], 199_999);
    }

    #[test]
    fn a_closed_chain_is_held_through_closed_issues() {
        // 0 and 1 are closed, 2 is open: 0 blocks 1, and 1 blocks 2. 3 is
        // closed, and only a closed issue waits for it.
        let open = [false, false, true, false];
        let held = held_by(&[(0, 1), (1, 2), (3, 0)], &open);
        assert_eq!(held, vec![true, true, false, true]);
        let held = held_by(&[(3, 0)], &open);
        assert_eq!(held, vec![false; 4]);
    }

    #[test]
    fn the_graph_keeps_blockers_in_key_order_and_skips_trashed_issues() {
        let s = SEP;
        // b2 and b10 block x. Key order puts number 2 before 10. "gone" is
        // trashed, so it is not in the nodes.
        let nodes = format!("x{s}t{s}1{s}1{s}b10{s}t{s}10{s}1{s}b2{s}t{s}2{s}1");
        let edges = format!("b10{s}x{s}b2{s}x{s}gone{s}x");
        let on_edges: HashSet<&str> = edges.split(s).collect();
        let g = Graph::build(Vec::new(), &edges, &nodes, &on_edges).unwrap();
        let x = g.of("x").unwrap();
        let names: Vec<&str> = g.blockers[x]
            .iter()
            .map(|&b| g.ids[b as usize].as_str())
            .collect();
        assert_eq!(names, vec!["b2", "b10"]);
        assert_eq!(g.depth[x], 1);
    }

    #[test]
    fn the_shortest_path_names_each_step() {
        let next = HashMap::from([("a", vec!["b", "x"]), ("b", vec!["c"]), ("x", vec!["c"])]);
        assert_eq!(shortest_path(&next, "a", "c").unwrap().len(), 3);
        assert_eq!(shortest_path(&next, "c", "a"), None);
    }
}
