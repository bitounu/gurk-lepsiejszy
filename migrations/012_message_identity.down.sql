-- Equal protocol timestamps cannot fit the old primary key. Refuse a lossy rollback.
CREATE TEMP TABLE identity_rollback_guard (collisions INTEGER CHECK (collisions = 0));
INSERT INTO identity_rollback_guard SELECT COUNT(*) FROM message_identity WHERE local_id != protocol_time;
DROP TABLE identity_rollback_guard;
DROP TABLE message_identity;
