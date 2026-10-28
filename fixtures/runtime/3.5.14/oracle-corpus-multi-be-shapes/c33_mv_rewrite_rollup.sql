SELECT s_cust, SUM(s_amount) AS amount, SUM(s_qty) AS qty FROM sales WHERE s_cust <= 200 GROUP BY s_cust ORDER BY amount DESC, s_cust LIMIT 10;
