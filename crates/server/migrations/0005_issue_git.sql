-- Git details kept on the issue, so they outlive the claim: the branch, the
-- worktree and its host, and a PR link set by hand.
ALTER TABLE issues ADD COLUMN git_branch TEXT;
ALTER TABLE issues ADD COLUMN git_worktree TEXT;
ALTER TABLE issues ADD COLUMN git_host TEXT;
ALTER TABLE issues ADD COLUMN git_pr TEXT;

-- Start each issue from its latest claim.
UPDATE issues SET
    git_branch = (SELECT c.branch FROM claims c WHERE c.issue_id = issues.id ORDER BY c.created_at DESC, c.id DESC LIMIT 1),
    git_worktree = (SELECT c.worktree FROM claims c WHERE c.issue_id = issues.id ORDER BY c.created_at DESC, c.id DESC LIMIT 1),
    git_host = (SELECT c.host FROM claims c WHERE c.issue_id = issues.id ORDER BY c.created_at DESC, c.id DESC LIMIT 1)
WHERE EXISTS (SELECT 1 FROM claims c WHERE c.issue_id = issues.id);
