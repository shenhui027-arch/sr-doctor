SELECT c.c_name, SUM(s.s_amount) AS total FROM sales s JOIN customers c ON s.s_cust = c.c_id GROUP BY c.c_name ORDER BY total DESC, c.c_name LIMIT 10;
