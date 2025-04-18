CREATE TABLE IF NOT EXISTS Websites (
    id serial primary key,
    url varchar not null,
    alias varchar(75) not null unique,
    created_at timestampz not null DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS Logs (
    id serial primary key,
    website_id int NOT null REFERENCES Websites(id),
    status smallint,
    error_msg varchar NOT NULL,
    created_at timestamp not null default date_trunc('minute', current_timestamp),
    UNIQUE(website_id, created_at)
);
