CREATE DATABASE IF NOT EXISTS oracle_corpus;
USE oracle_corpus;
CREATE EXTERNAL TABLE IF NOT EXISTS ext_customers (c_id INT, c_name VARCHAR(32), c_region INT, c_segment VARCHAR(8)) ENGINE=MYSQL PROPERTIES ('host'='127.0.0.1', 'port'='9030', 'user'='root', 'password'='', 'database'='oracle_corpus', 'table'='customers');
SELECT 'ext_customers', COUNT(*) FROM ext_customers;
