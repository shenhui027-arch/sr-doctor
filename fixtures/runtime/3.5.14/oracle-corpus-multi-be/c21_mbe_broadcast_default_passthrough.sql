SELECT w.w_tag % 10 AS tag_bucket, COUNT(*) AS cnt, MAX(w.w_payload) AS max_payload FROM sales s JOIN [BROADCAST] wide_dim w ON s.s_id = w.w_id GROUP BY w.w_tag % 10;
