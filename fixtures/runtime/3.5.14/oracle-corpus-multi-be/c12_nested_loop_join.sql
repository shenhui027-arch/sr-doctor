SELECT COUNT(*) AS cnt FROM products p JOIN customers c ON p.p_price > c.c_region * 10 + 900 WHERE c.c_id < 600;
