SET pipeline_dop=4;
SET parallel_fragment_exec_instance_num=4;
SELECT d.d_attr, COUNT(*) AS cnt, SUM(f.f_val) AS val FROM skew_fact f JOIN [SHUFFLE] skew_dim d ON f.f_key = d.d_key AND concat(d.d_name, '_', CAST(f.f_val AS VARCHAR)) <> 'none' GROUP BY d.d_attr;
