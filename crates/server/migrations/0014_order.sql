-- LLM-206: an epic's sub-issues get an order in steps. A step number is
-- scoped to the parent, and the steps of one parent stay dense, 1..n.
ALTER TABLE issues ADD COLUMN order_step INTEGER CHECK (order_step IS NULL OR order_step > 0);
CREATE INDEX issues_parent_order ON issues (parent_id, order_step);

-- The order works as dependencies: the server writes a `blocks` edge from
-- each task of step N to each task of step N+1, with source `order`, and
-- rewrites them on each order change. Every other edge is `manual`.
ALTER TABLE relations ADD COLUMN source TEXT NOT NULL DEFAULT 'manual' CHECK (source IN ('manual', 'order'));
