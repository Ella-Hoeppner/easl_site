-- Page views, one row each (see `src/visits.rs`): when, on which site
-- ('docs' or 'editor'), the visitor's country (an ISO 3166-1 code, NULL
-- when unknown), and the visitor's hash for that day. Addresses are never
-- stored.

CREATE TABLE visits (
  id INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  site TEXT NOT NULL CHECK (site IN ('docs', 'editor')),
  country TEXT,
  visitor TEXT NOT NULL
);

CREATE INDEX visits_by_time ON visits (at);
