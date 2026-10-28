/**
 * One short form for StarRocks query IDs in the query lists: the first 8 and
 * last 4 characters. The first segment tells executions apart and the tail
 * tells hosts apart; callers keep the full ID in a title.
 */
export function compactQueryId(queryId: string): string {
  return queryId.length > 14 ? `${queryId.slice(0, 8)}…${queryId.slice(-4)}` : queryId;
}
