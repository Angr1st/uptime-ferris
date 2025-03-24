CREATE TABLE IF NOT EXISTS Websites (
    id serial primary key,
    url varchar not null,
    alias varchar(75) not null unique,
    created_at timestamp without time zone not null,
    created_at_user timestampz not null,
    updated_at timestamp without time zone not null,
    updated_at_user timestampz not null,
);

CREATE TABLE IF NOT EXISTS Logs (
    id serial primary key,
    website_id int NOT null REFERENCES Websites(id),
    status smallint,
    error_msg varchar(MAX) NOT NULL,
    created_at timestamp without time zone not null default date_trunc('minute', current_timestamp),
    UNIQUE(website_id, created_at)
);
