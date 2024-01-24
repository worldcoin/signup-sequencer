# Database migration files

These migration scripts are statically linked into the application.

They must
be names `<VERSION>_<DESCRIPTION>.{up|down}.sql`.

Migrations are tracked and executed using `sqlx`.

For manual management/inspection/testing [sqlx-cli](https://github.com/launchbadge/sqlx/tree/master/sqlx-cli) can be used.
