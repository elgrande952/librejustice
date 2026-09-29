ALTER TABLE users ADD COLUMN mcp_connected_at timestamptz;

UPDATE users u
   SET mcp_connected_at = least(
           (SELECT min(t.created_at) FROM mcp_tokens t WHERE t.user_id = u.sub),
           (SELECT min(h.created_at) FROM user_search_history h
             WHERE h.user_sub = u.sub AND h.source = 'mcp'))
 WHERE EXISTS (SELECT 1 FROM mcp_tokens t WHERE t.user_id = u.sub)
    OR EXISTS (SELECT 1 FROM user_search_history h
                WHERE h.user_sub = u.sub AND h.source = 'mcp');
