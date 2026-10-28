SELECT COUNT(*) AS cnt, SUM(s_amount) AS amount FROM sales WHERE s_amount > (SELECT AVG(s_amount) * 1.5 FROM sales WHERE s_qty > 5);
