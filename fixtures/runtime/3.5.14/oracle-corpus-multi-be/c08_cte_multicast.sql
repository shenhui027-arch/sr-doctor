SET cbo_cte_reuse = true;
SET cbo_cte_reuse_rate = 0;
WITH t AS (SELECT s_cust, SUM(s_amount) AS amt, COUNT(*) AS cnt FROM sales GROUP BY s_cust) SELECT COUNT(*) AS pairs, SUM(a.amt - b.amt) AS diff FROM t a JOIN t b ON a.s_cust = b.s_cust + 1 WHERE a.cnt > 5;
