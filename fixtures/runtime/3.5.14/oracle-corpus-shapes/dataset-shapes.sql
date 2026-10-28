CREATE DATABASE IF NOT EXISTS oracle_corpus;
USE oracle_corpus;
CREATE MATERIALIZED VIEW IF NOT EXISTS mv_sales_cust_day DISTRIBUTED BY HASH(s_cust) BUCKETS 2 REFRESH MANUAL PROPERTIES ('replication_num'='1') AS SELECT s_cust, s_date, SUM(s_amount) AS amount, SUM(s_qty) AS qty, COUNT(*) AS cnt FROM sales GROUP BY s_cust, s_date;
REFRESH MATERIALIZED VIEW mv_sales_cust_day WITH SYNC MODE;
INSERT INTO FILES('path' = 'file:///tmp/oracle_shapes/sales_parquet/', 'format' = 'parquet') SELECT s_id, s_date, s_cust, s_prod, s_qty, s_amount FROM sales WHERE s_id <= 50000;
SELECT 'mv_sales_cust_day', COUNT(*) FROM mv_sales_cust_day UNION ALL SELECT 'sales_parquet', COUNT(*) FROM FILES('path' = 'file:///tmp/oracle_shapes/sales_parquet/*', 'format' = 'parquet');
