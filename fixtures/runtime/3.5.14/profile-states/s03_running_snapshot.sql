SELECT '__MARKER__' AS run_marker, s_cust % 5 AS k, COUNT(*) AS cnt, SUM(SLEEP(1)) AS slept FROM sales WHERE s_id <= 160 GROUP BY s_cust % 5;
