SELECT COUNT(*) AS cnt, MAX(running) AS max_running FROM (SELECT s_cust, SUM(s_amount) OVER (PARTITION BY s_cust ORDER BY s_id) AS running FROM sales WHERE s_cust < 4000) t;
