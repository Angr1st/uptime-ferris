pub const SELECT_MONTHLY_STATS: &str = r#"
                Select date_trunc('day', Logs.created_at) as time,
                CAST(COUNT(case when status = 200 then 1 end) * 100 / COUNT(*) AS int2) AS uptime_pct
                FROM Logs
                LEFT JOIN Websites ON Websites.id = Logs.website_id
                WHERE Websites.alias = $1
                GROUP BY time
                ORDER BY time asc
                LIMIT 30
            "#;
pub const SELECT_DAILY_STATS: &str = r#"
                SELECT date_trunc('hour', Logs.created_at) as time,
                CAST(COUNT(case when status = 200 then 1 end) * 100 / COUNT(*) as int2) as uptime_pct
                FROM Logs
                LEFT JOIN Websites on Websites.id = Logs.website_id
                WHERE Websites.alias = $1
                GROUP BY time
                ORDER BY time asc
                LIMIT 24
                "#;
