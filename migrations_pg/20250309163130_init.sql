CREATE TABLE IF NOT EXISTS Websites (
    id serial primary key,
    url varchar not null,
    alies varchar(75) not null unique
);

CREATE TABLE IF NOT EXISTS Logs (
    id serial primary key,
    website_id int NOT null REFERENCES Websites(id),
    status smallint,
    created_at timestamp with time zone not null default date_trunc('minute', current_timestamp),
    UNIQUE(website_id, created_at)
);
