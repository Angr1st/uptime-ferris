CREATE TABLE IF NOT EXISTS Users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    salt TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT 'now'
);

CREATE TABLE IF NOT EXISTS Permissions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    description TEXT
);

CREATE TABLE IF NOT EXISTS User_Permissions (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    website_id INTEGER NOT NULL,
    permission_id INTEGER NOT NULL,
    FOREIGN KEY (user_id) REFERENCES Users (id) ON DELETE CASCADE,
    FOREIGN KEY (website_id) REFERENCES Websites (id) ON DELETE CASCADE,
    FOREIGN KEY (permission_id) REFERENCES Permissions (id),
    UNIQUE (user_id, website_id, permission_id)
);

-- Insert the two permission types
INSERT INTO Permissions (name, description) VALUES 
    ('read', 'Can read the Website'),
    ('create_modify', 'Can create and modify the Website');
