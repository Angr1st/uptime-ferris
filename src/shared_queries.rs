pub const INSERT_INTO_WEBSITES_QUERY: &str =
    "INSERT INTO Websites (url, alias, created_at) VALUES ($1,$2,$3)";
pub const SELECT_URL_ALIAS_WEBSITES_QUERY: &str = "SELECT url, alias FROM Websites";
pub const SELECT_URL_ALIAS_WEBSITES_TOP_ONE_WHERE_ALIAS_QUERY: &str =
    "SELECT url, alias FROM Websites WHERE alias = $1 LIMIT 1";
pub const SELECT_INCIDENTS_BY_WEBSITE_ALIAS_QUERY: &str = "
            SELECT Logs.created_at as time,
            Logs.status from Logs
            LEFT JOIN Websites on Websites.id = Logs.website_id
            where Websites.Alias = $1 and Logs.status <> 200
            ";
pub const DELETE_LOGS_BY_WEBSITE_ALIAS_QUERY: &str = "DELETE FROM Logs WHERE id IN
        (SELECT Logs.id
        FROM Logs
        LEFT JOIN Websites ON Websites.id = Logs.website_id
        WHERE Websites.alias = $1)";
pub const DELETE_WEBSITE_BY_ALIAS_QUERY: &str = "DELETE FROM Websites WHERE alias = $1";
pub const INSERT_INTO_LOGS_BY_ALIAS_RESPONSE_CODE_QUERY: &str = r#"INSERT INTO Logs (website_id, status)
                VALUES
                ((SELECT id FROM Websites WHERE alias = $1), $2)"#;
