DO $$
BEGIN
    EXECUTE format('ALTER DATABASE %I SET paradedb.enable_aggregate_custom_scan = off',
                   current_database());
END $$;
