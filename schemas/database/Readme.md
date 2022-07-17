# Database migration files

These migration scripts are statically linked into the application. They must
be names `<VERSION>_<DESCRIPTION>.sql` for simple migrations. Complex migrations
can be done using `.up.sql` and `.down.sql` scripts.

Migrations are tracked and executed using `sqlx`.
