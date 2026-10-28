CREATE DATABASE IF NOT EXISTS oracle_corpus;
USE oracle_corpus;
CREATE TABLE IF NOT EXISTS skew_tablet (t_id BIGINT NOT NULL, t_key INT NOT NULL, t_val INT NOT NULL) ENGINE=OLAP DUPLICATE KEY (t_id) DISTRIBUTED BY HASH(t_key) BUCKETS 6 PROPERTIES ('replication_num'='1');
INSERT INTO skew_tablet SELECT g, CASE WHEN g % 20 = 0 THEN CAST(g % 5000 + 2 AS INT) ELSE 1 END, CAST(g % 1000 AS INT) FROM (SELECT CAST(generate_series AS BIGINT) AS g FROM TABLE(generate_series(1, 6000000))) x;
ANALYZE FULL TABLE skew_tablet WITH SYNC MODE;
SELECT 'skew_tablet', COUNT(*) FROM skew_tablet;
