SELECT COUNT(*) AS cnt FROM customers c WHERE c.c_id IN (SELECT r_cust FROM returns WHERE r_amount > 10) AND NOT EXISTS (SELECT 1 FROM events e WHERE e.e_cust = c.c_id AND e.e_type = 'purchase');
