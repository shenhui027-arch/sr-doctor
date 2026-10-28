SELECT t.part, COUNT(*) AS cnt FROM customers, unnest(split(c_name, '_')) AS t(part) WHERE c_region < 3 GROUP BY t.part ORDER BY cnt DESC, t.part LIMIT 5;
