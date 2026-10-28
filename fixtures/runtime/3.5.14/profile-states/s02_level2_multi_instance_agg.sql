SET pipeline_dop=1;
SET parallel_fragment_exec_instance_num=2;
SELECT s_prod % 50 AS prod_bucket, COUNT(*) AS cnt, SUM(s_amount) AS amount FROM sales GROUP BY s_prod % 50;
