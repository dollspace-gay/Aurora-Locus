-- v0.9 Arc E 0.9.3 / chainlink #272 — refresh-token rotation-on-use.
--
-- Adds the rotation timestamp the grace window needs. #271 staged
-- current_refresh_id / prev_refresh_id (the rotation chain head + its
-- immediate predecessor) on operator_session; rotation-on-use advances
-- them on every refresh. `refresh_rotated_at` records WHEN current last
-- advanced, so the predecessor token (prev_refresh_id) is honoured only
-- within a brief grace window — long enough to cover a client that
-- refreshed but lost the HTTP response and retried, short enough that a
-- leaked old token dies promptly. A separate column (not revoked_at) keeps
-- rotation distinct from force-logout (#273), which is a different event.
--
-- TEXT (RFC3339), nullable: NULL until the session's first rotation.

ALTER TABLE operator_session ADD COLUMN refresh_rotated_at TEXT;
