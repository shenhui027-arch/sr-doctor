SELECT c.c_region, COUNT(*) AS cnt, SUM(s.s_amount) AS amount FROM sales s JOIN [SHUFFLE] customers c ON s.s_cust = c.c_id GROUP BY c.c_region;
