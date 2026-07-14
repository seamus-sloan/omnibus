-- #865: cap how many devices a single user can accumulate. Devices grow
-- only through the user's own repeated logins (not attacker-controlled bulk
-- insertion the way a comment/tag field would be), so a trigger-based,
-- evict-oldest-on-overflow cap is a low-risk fit — it runs atomically with
-- every INSERT regardless of call site (pool or transaction executor), so
-- it needs no change to `register_device`'s generic `Executor` signature.
--
-- The cap (50) must stay in sync with `MAX_DEVICES_PER_USER` in
-- `db/src/auth/device.rs` — a test asserts the two agree.
CREATE TRIGGER trg_devices_cap_per_user AFTER INSERT ON devices
BEGIN
    DELETE FROM devices
     WHERE user_id = NEW.user_id
       AND id NOT IN (
           SELECT id FROM devices
            WHERE user_id = NEW.user_id
            ORDER BY last_seen_at DESC, id DESC
            LIMIT 50
       );
END;
