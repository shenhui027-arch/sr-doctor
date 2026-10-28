SELECT t_key % 7 AS k, COUNT(*) AS cnt, SUM(murmur_hash3_32(concat(CAST(t_val AS VARCHAR), '_', CAST(t_id AS VARCHAR)))) AS h FROM skew_tablet GROUP BY t_key % 7;
