SELECT s_cust % 10 AS bucket, COUNT(*) AS cnt, SUM(s_amount) AS amount FROM FILES('path' = 'file:///tmp/oracle_shapes/sales_parquet/*', 'format' = 'parquet') GROUP BY s_cust % 10 ORDER BY bucket;
