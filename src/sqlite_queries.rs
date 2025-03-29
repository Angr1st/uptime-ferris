pub const SELECT_MONTHLY_STATS: &str = r#"
                SELECT strftime('%Y-%m-%d 00:00:00', Logs.created_at) as time,
                CAST(COUNT(CASE WHEN status = 200 THEN 1 END) * 100 / COUNT(*) AS INTEGER) as uptime_pct
                FROM Logs
                LEFT JOIN Websites ON Websites.id = Logs.website_id
                WHERE Websites.alias = $1
                GROUP BY time
                ORDER BY time ASC
                LIMIT 30
            "#;
pub const SELECT_DAILY_STATS: &str = r#"
                SELECT strftime('%Y-%m-%d %H:00:00', Logs.created_at) as time,
                CAST(COUNT(CASE WHEN status = 200 THEN 1 END) * 100 / COUNT(*) AS INTEGER) as uptime_pct
                FROM Logs
                LEFT JOIN Websites ON Websites.id = Logs.website_id
                WHERE Websites.alias = $1
                GROUP BY time
                ORDER BY time ASC
                LIMIT 24
                "#;
