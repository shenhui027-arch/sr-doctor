SELECT f.f_key % 10 AS key_bucket, COUNT(*) AS cnt, SUM(l.l_val + f.f_val) AS val FROM fan_left l JOIN [SHUFFLE] skew_fact f ON l.l_id = f.f_id GROUP BY f.f_key % 10;
