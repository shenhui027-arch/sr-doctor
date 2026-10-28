SET new_planner_agg_stage=2;
SET enable_local_shuffle_agg=false;
SELECT COUNT(*) AS group_cnt, SUM(t.cnt) AS row_total, MAX(t.val) AS max_val FROM (SELECT h_user, COUNT(*) AS cnt, SUM(h_val) AS val FROM hc_fact GROUP BY h_user) t;
