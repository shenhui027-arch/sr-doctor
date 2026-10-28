// Read-only view of a stage B history file for the E2E receipt: per-template
// execution totals for one database and whether any slice was read twice.
// Usage: node inspect-history.mjs <history.sqlite> <database>
import { DatabaseSync } from "node:sqlite";

const [path, database] = process.argv.slice(2);
if (!path || !database) throw new Error("usage: inspect-history.mjs <history.sqlite> <database>");
const db = new DatabaseSync(path, { readOnly: true });
const templates = db.prepare(`
  SELECT t.kind, t.template, SUM(b.executions) AS executions, SUM(b.errors) AS errors, SUM(b.qt_valid) AS qt_valid, COUNT(*) AS buckets
  FROM templates t JOIN buckets b ON b.template_key = t.template_key
  WHERE t.database = ? GROUP BY t.template_key ORDER BY executions DESC, t.template`).all(database);
const reread = db.prepare(`
  SELECT COUNT(*) AS n FROM (SELECT target, slice_start FROM runs WHERE state IN ('complete', 'partial')
  GROUP BY target, slice_start HAVING COUNT(*) > 1)`).get();
const runs = db.prepare("SELECT state, COUNT(*) AS n FROM runs GROUP BY state ORDER BY state").all();
db.close();
process.stdout.write(JSON.stringify({
  templates: templates.map((row) => ({ kind: row.kind, template: row.template, executions: Number(row.executions), errors: Number(row.errors), qt_valid: Number(row.qt_valid), buckets: Number(row.buckets) })),
  reread_slices: Number(reread.n),
  runs: Object.fromEntries(runs.map((row) => [row.state, Number(row.n)])),
}));
