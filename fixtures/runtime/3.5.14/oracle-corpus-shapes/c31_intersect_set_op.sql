SELECT COUNT(*) AS cnt FROM (SELECT s_cust FROM sales WHERE s_qty = 10 INTERSECT SELECT r_cust FROM returns INTERSECT SELECT e_cust FROM events WHERE e_type = 'click') t;
