SELECT COUNT(*) AS cnt, SUM(t.rn) AS rn_total FROM (SELECT ROW_NUMBER() OVER (PARTITION BY f_val ORDER BY f_id) AS rn FROM skew_fact) t;
