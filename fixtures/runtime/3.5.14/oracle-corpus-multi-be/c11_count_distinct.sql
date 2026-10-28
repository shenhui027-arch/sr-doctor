SELECT s_prod % 20 AS prod_bucket, COUNT(DISTINCT s_cust) AS customers, COUNT(*) AS cnt FROM sales GROUP BY s_prod % 20;
