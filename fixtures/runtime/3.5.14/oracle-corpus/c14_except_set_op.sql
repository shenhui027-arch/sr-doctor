SELECT COUNT(*) AS cnt FROM (SELECT s_cust FROM sales WHERE s_qty = 10 EXCEPT SELECT r_cust FROM returns) t;
