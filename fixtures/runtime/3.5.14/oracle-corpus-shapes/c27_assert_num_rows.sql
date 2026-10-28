SELECT COUNT(*) AS cnt, SUM(s_amount) AS amount FROM sales WHERE s_cust = (SELECT c_id FROM customers WHERE c_name = 'cust_77');
