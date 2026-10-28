SET enable_exchange_pass_through=false;
SELECT l.l_grp, COUNT(*) AS cnt, SUM(r.r_val) AS val FROM fan_left l JOIN fan_right r ON l.l_key = r.r_key GROUP BY l.l_grp;
