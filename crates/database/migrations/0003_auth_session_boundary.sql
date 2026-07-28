-- Authentication starts with an opaque cookie token, before the application knows user_id.
-- Narrow SECURITY DEFINER functions are the only exception to tenant RLS: they expose no
-- session listing and only operate on the exact SHA-256 token hash supplied by the caller.
CREATE OR REPLACE FUNCTION authenticate_session(p_token_hash text, p_now timestamptz)
RETURNS TABLE(token_hash text, user_id text, expires_at timestamptz)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
  SELECT s.token_hash, s.user_id, s.expires_at
  FROM public.auth_sessions s
  WHERE s.token_hash = p_token_hash AND s.expires_at > p_now
  LIMIT 1
$$;

CREATE OR REPLACE FUNCTION refresh_auth_session(p_token_hash text, p_expires_at timestamptz)
RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
BEGIN
  UPDATE public.auth_sessions SET expires_at = p_expires_at, last_seen_at = now()
  WHERE token_hash = p_token_hash;
  RETURN FOUND;
END $$;

CREATE OR REPLACE FUNCTION delete_auth_session(p_token_hash text)
RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
BEGIN
  DELETE FROM public.auth_sessions WHERE token_hash = p_token_hash;
  RETURN FOUND;
END $$;

CREATE OR REPLACE FUNCTION prune_auth_sessions(p_now timestamptz)
RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
DECLARE affected bigint;
BEGIN
  DELETE FROM public.auth_sessions WHERE expires_at <= p_now;
  GET DIAGNOSTICS affected = ROW_COUNT;
  RETURN affected;
END $$;
