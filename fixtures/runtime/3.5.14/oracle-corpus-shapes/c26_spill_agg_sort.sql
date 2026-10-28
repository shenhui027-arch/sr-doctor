SET enable_spill=true;
SET spill_mode='force';
EXPLAIN ANALYZE SELECT s_cust, COUNT(*) AS cnt, SUM(s_amount) AS amount FROM sales GROUP BY s_cust ORDER BY amount DESC, s_cust;
