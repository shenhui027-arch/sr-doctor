SELECT a.bucket, SUM(b.id + c.id + d.id) AS total
FROM explain_fixture.rows4 a
JOIN [BROADCAST] explain_fixture.rows4 b ON a.bucket = b.bucket
JOIN [SHUFFLE] explain_fixture.rows4 c ON a.id = c.id
JOIN [BROADCAST] explain_fixture.rows4 d ON c.bucket = d.bucket
WHERE a.id > 0
GROUP BY a.bucket
ORDER BY total DESC
LIMIT 10;