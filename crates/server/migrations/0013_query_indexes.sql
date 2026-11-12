-- The default issue sort: priority with no priority last, then the newest
-- update, then the ID. The expression must match SORT_PRIORITY in
-- store/issues.rs, or the planner cannot use the index.
CREATE INDEX issues_default_sort ON issues (
    (CASE WHEN priority = 0 THEN 5 ELSE priority END),
    updated_at DESC,
    id
) WHERE deleted_at IS NULL;

-- The GitHub poll finds the issues that hold a pull request by its URL.
CREATE INDEX attachments_url ON attachments (url);

-- A document's history reads its activity by entity, in seq order.
CREATE INDEX activity_entity ON activity (entity_id, seq);
