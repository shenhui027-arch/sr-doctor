SELECT s_prod % 50 AS prod_bucket, COUNT(*) AS cnt, SUM(s_amount) AS amount FROM sales GROUP BY s_prod % 50;
