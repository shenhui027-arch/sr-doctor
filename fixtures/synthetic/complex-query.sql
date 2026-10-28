WITH filtered_sales AS (
  SELECT event_id, customer_id, product_id, channel, amount
  FROM runtime_corpus.sales
  WHERE event_id >= 1 AND event_id < 50001
), return_totals AS (
  SELECT event_id, SUM(refund) AS refund_sum
  FROM runtime_corpus.returns
  WHERE return_id >= 1 AND return_id < 25001
    AND event_id >= 1 AND event_id < 50001
  GROUP BY event_id
), grouped AS (
  SELECT c.region, p.category, s.channel,
         COUNT(*) AS occurrence_count,
         SUM(s.amount) AS amount_sum,
         SUM(COALESCE(r.refund_sum, 0)) AS refund_sum,
         SUM(s.amount - COALESCE(r.refund_sum, 0)) AS difference_sum
  FROM filtered_sales s
  JOIN runtime_corpus.customers c ON s.customer_id = c.customer_id
  JOIN runtime_corpus.products p ON s.product_id = p.product_id
  LEFT JOIN return_totals r ON s.event_id = r.event_id
  WHERE c.customer_id >= 1 AND c.customer_id < 20001
    AND p.product_id >= 1 AND p.product_id < 5001
  GROUP BY c.region, p.category, s.channel
), ranked AS (
  SELECT region, category, channel, occurrence_count,
         amount_sum, refund_sum, difference_sum,
         DENSE_RANK() OVER (
           PARTITION BY region ORDER BY difference_sum DESC
         ) AS amount_rank
  FROM grouped
)
SELECT region, category, channel, occurrence_count,
       amount_sum, refund_sum, difference_sum, amount_rank
FROM ranked
WHERE amount_rank <= 3
ORDER BY region, amount_rank, category, channel
LIMIT 60