//! Dependencies between issues. A dependency is a `blocks` relation: "A
//! depends on B" is stored as "B blocks A". Depth is the longest chain of
//! open blockers.
//!
//! One recursive query collects the blocker edges a set of issues can reach.
//! The longest paths are then computed in Rust, once per node. A recursive
//! query that counted lengths in SQL would walk every path, and the number
//! of paths grows exponentially when chains share blockers.
//!
//! The queries use CROSS JOIN, which makes SQLite keep the written join
//! order. Left to itself, its planner starts from `statuses` and walks every
//! issue for each step of the recursion. On 50,000 issues that took 25
//! seconds per page. The walk up needs the index from migration 0007 for
//! the same reason: without it, each step scans every `blocks` row.

use std::collections::{HashMap, HashSet, VecDeque};

use llmtodo_api_types::{DepNode, IssueKey, IssueLink};
use sqlx::SqliteConnection;

use crate::error::{ApiError, ApiResult};
use crate::store::{issues, teams};

/// How deep `issue deps` goes. A node takes two levels of JSON nesting, and
/// serde refuses to read more than 128.
const MAX_TREE_DEPTH: usize = 40;

/// The open blockers of a set of issues, and of their blockers in turn.
#[derive(Default)]
pub struct Blockers {
    /// Blockers of each issue that has any, in key order.
    edges: HashMap<String, Vec<String>>,
    links: HashMap<String, IssueLink>,
    depth: HashMap<String, u32>,
}

#[derive(sqlx::FromRow)]
struct EdgeRow {
    issue: String,
    blocker: String,
    key: String,
    title: String,
    status_id: String,
}

impl Blockers {
    /// Loads the chains of the open, live issues in `ids`. A closed or
    /// trashed issue has no chain. The query visits each reachable issue
    /// once, so its cost follows the size of the chains, not of the database.
    pub async fn load(
        conn: &mut SqliteConnection,
        ids: &[String],
        statuses: &HashMap<String, teams::StatusRow>,
    ) -> ApiResult<Self> {
        let rows = sqlx::query_as::<_, EdgeRow>(
            "WITH RECURSIVE reach(id) AS (
                SELECT i.id FROM json_each(?) j
                CROSS JOIN issues i ON i.id = j.value
                CROSS JOIN statuses s ON s.id = i.status_id
                WHERE i.deleted_at IS NULL AND s.type NOT IN ('completed', 'canceled')
                UNION
                SELECT b.id FROM reach
                CROSS JOIN relations r ON r.to_id = reach.id AND r.type = 'blocks'
                CROSS JOIN issues b ON b.id = r.from_id
                CROSS JOIN statuses s ON s.id = b.status_id
                WHERE b.deleted_at IS NULL AND s.type NOT IN ('completed', 'canceled')
             )
             SELECT r.to_id AS issue, b.id AS blocker, b.key, b.title, b.status_id
             FROM reach
             CROSS JOIN relations r ON r.to_id = reach.id AND r.type = 'blocks'
             CROSS JOIN issues b ON b.id = r.from_id
             CROSS JOIN statuses s ON s.id = b.status_id
             WHERE b.deleted_at IS NULL AND s.type NOT IN ('completed', 'canceled')
             ORDER BY b.team_id, b.number",
        )
        .bind(serde_json::to_string(ids)?)
        .fetch_all(&mut *conn)
        .await?;
        let mut out = Self::default();
        for r in rows {
            if !out.links.contains_key(&r.blocker) {
                let status = statuses
                    .get(&r.status_id)
                    .map(super::teams::StatusRow::to_ref)
                    .ok_or_else(|| ApiError::internal("issue has an unknown status"))?;
                let key: IssueKey = r
                    .key
                    .parse()
                    .map_err(|_| ApiError::internal(format!("bad key {}", r.key)))?;
                out.links.insert(
                    r.blocker.clone(),
                    IssueLink {
                        key,
                        title: r.title,
                        status,
                        deleted: false,
                    },
                );
            }
            out.edges.entry(r.issue).or_default().push(r.blocker);
        }
        out.depth = chain_depths(&out.edges);
        Ok(out)
    }

    pub fn depth(&self, id: &str) -> u32 {
        self.depth.get(id).copied().unwrap_or(0)
    }

    pub fn open_blockers(&self, id: &str) -> Vec<IssueLink> {
        self.edges.get(id).map_or_else(Vec::new, |blockers| {
            blockers
                .iter()
                .filter_map(|b| self.links.get(b).cloned())
                .collect()
        })
    }

    /// The keys along the longest chain, from the nearest blocker to the
    /// farthest. Between equal chains, the lowest key wins.
    pub fn longest_chain(&self, id: &str) -> Vec<IssueKey> {
        let mut chain = Vec::new();
        let mut seen = HashSet::from([id]);
        let mut current = id;
        while let Some(next) = self.edges.get(current).and_then(|blockers| {
            // `max_by_key` keeps the last of equal items, so walk in reverse.
            blockers.iter().rev().max_by_key(|b| self.depth(b))
        }) {
            if !seen.insert(next) {
                break;
            }
            if let Some(link) = self.links.get(next) {
                chain.push(link.key.clone());
            }
            current = next;
        }
        chain
    }
}

/// The longest path below each node. It runs on an explicit stack, so a very
/// long chain cannot overflow the thread's stack. An edge back onto the
/// current path is ignored, so a cycle in old data cannot hang it.
fn chain_depths(edges: &HashMap<String, Vec<String>>) -> HashMap<String, u32> {
    let mut depth: HashMap<String, u32> = HashMap::with_capacity(edges.len());
    let mut on_path: HashSet<&str> = HashSet::new();
    let mut stack: Vec<(&str, usize)> = Vec::new();
    for root in edges.keys() {
        if depth.contains_key(root) {
            continue;
        }
        on_path.insert(root);
        stack.push((root, 0));
        while let Some((node, next)) = stack.last_mut() {
            let node = *node;
            let blockers = edges.get(node).map_or(&[][..], Vec::as_slice);
            if let Some(blocker) = blockers.get(*next) {
                *next += 1;
                if !depth.contains_key(blocker) && on_path.insert(blocker) {
                    stack.push((blocker, 0));
                }
                continue;
            }
            let d = blockers
                .iter()
                .filter_map(|b| depth.get(b))
                .map(|d| d + 1)
                .max()
                .unwrap_or(0);
            depth.insert(node.to_string(), d);
            on_path.remove(node);
            stack.pop();
        }
    }
    depth
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
    let links = issues::link_map(conn, &ids).await?;
    let statuses: HashMap<String, teams::StatusRow> = teams::statuses(conn, None)
        .await?
        .into_iter()
        .map(|s| (s.id.clone(), s))
        .collect();
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

    fn edges(list: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        list.iter()
            .map(|(a, b)| (a.to_string(), b.iter().map(ToString::to_string).collect()))
            .collect()
    }

    #[test]
    fn depth_is_the_longest_chain() {
        let e = edges(&[("a", &["b"]), ("b", &["c"])]);
        let d = chain_depths(&e);
        assert_eq!((d["a"], d["b"], d["c"]), (2, 1, 0));
    }

    #[test]
    fn a_shared_blocker_counts_by_the_longest_way() {
        // a waits for b and c, and b also waits for c.
        let e = edges(&[("a", &["c", "b"]), ("b", &["c"])]);
        assert_eq!(chain_depths(&e)["a"], 2);
    }

    #[test]
    fn a_cycle_in_old_data_ends() {
        let e = edges(&[("a", &["b"]), ("b", &["a"])]);
        let d = chain_depths(&e);
        assert!(d["a"] <= 1 && d["b"] <= 1);
    }

    #[test]
    fn a_very_long_chain_does_not_overflow() {
        let names: Vec<String> = (0..200_000).map(|n| format!("n{n}")).collect();
        let e: HashMap<String, Vec<String>> = names
            .windows(2)
            .map(|w| (w[0].clone(), vec![w[1].clone()]))
            .collect();
        assert_eq!(chain_depths(&e)["n0"], 199_999);
    }

    #[test]
    fn the_shortest_path_names_each_step() {
        let next = HashMap::from([("a", vec!["b", "x"]), ("b", vec!["c"]), ("x", vec!["c"])]);
        assert_eq!(shortest_path(&next, "a", "c").unwrap().len(), 3);
        assert_eq!(shortest_path(&next, "c", "a"), None);
    }
}
