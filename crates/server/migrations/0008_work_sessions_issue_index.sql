-- The issue list adds up token cost per issue, so it finds work sessions by
-- issue. The UNIQUE (session_id, issue_id) index cannot serve that.
CREATE INDEX work_sessions_issue ON work_sessions (issue_id);
