SELECT c_region, COUNT(*) AS cnt FROM ext_customers WHERE c_region < 5 GROUP BY c_region ORDER BY c_region;
