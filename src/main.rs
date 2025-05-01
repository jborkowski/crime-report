use anyhow::anyhow;
use chrono::naive::NaiveDate;
use chrono::{Datelike, Local};
use clap::{arg, command, Parser};
use futures::future;

mod activities;
mod docs;

/*
jemalloc used to be the default Rust allocator til circa November 2018. Here we explicitly opt back into it to avoid the abysmal musl allocator
*/

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(next_line_help = true)]
struct Cli {
    #[arg(long)]
    #[arg(short = 'y')]
    #[arg(default_value_t = Local::now().date_naive().year())]
    year: i32,
    #[arg(long)]
    #[arg(short = 'm')]
    #[arg(default_value_t = Local::now().date_naive().month())]
    month: u32,

    #[arg(long, default_value_t = String::from("restaumatic"))]
    owner: String,

    #[arg(long, short = 'U')]
    user: String,

    #[arg(long)]
    gh_token: Option<String>,
}

impl Cli {
    fn since(&self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.year, self.month, 1).unwrap_or_default()
    }

    fn until(&self) -> NaiveDate {
        let mut year = self.year;
        let mut month = self.month;
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }

        NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default()
    }

    fn gh_token(&self) -> String {
        match self.gh_token {
            None => match std::env::var("GH_TOKEN") {
                Ok(gh_token) => gh_token,
                Err(_) => {
                    eprintln!("GH_TOKEN is not provided as a command-line argument or environment variable");
                    std::process::exit(1);
                }
            },
            Some(ref gh_token) => gh_token.to_string(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    println!(
        "Fetching activities for user: '{}' in '{}' organization ({} - {})",
        cli.user,
        cli.owner,
        cli.since(),
        cli.until()
    );
    let octocrab = octocrab::Octocrab::builder()
        .personal_token(cli.gh_token())
        .build()
        .map_err(|err| anyhow!("Failed to build Octocrab client: {}", err))?;

    let repos: Vec<String> =
        activities::get_organization_repositories(&octocrab, &cli.owner).await?;

    use snafu::Backtrace;

    let activities = future::join_all(repos.into_iter().map(|repo| {
        let octocrab = octocrab.clone();
        let since = cli.since();
        let until = cli.until();
        let user = cli.user.clone();
        let owner = cli.owner.clone();
        tokio::task::spawn(async move {
            activities::list_activity(&octocrab, &owner, &repo, user, since, until).await
        })
    }))
    .await
    .into_iter()
    .map(|task_result| {
        task_result
            .map_err(|task_error| octocrab::Error::Other {
                source: Box::new(task_error),
                backtrace: Backtrace::disabled(),
            })
            .and_then(|activity_result| activity_result)
    })
    .collect::<octocrab::Result<Vec<_>>>()
    .map_err(|err| anyhow!("Failed to collect repository activities: {}", err))?;

    create_from_template(&activities).await?;

    for activity in activities
        .iter()
        .filter(|activity| !activity.activities.is_empty())
    {
        print!("{}", activity);
    }

    Ok(())
}

async fn create_from_template(activities: &Vec<activities::Activity>) -> anyhow::Result<()> {
    let credentials_path = "credentials.json";

    let (docs, drive) = docs::create_clients(credentials_path)
        .await
        .map_err(|err| anyhow!("Error creating document clients: {}", err))?;

    let template_id = "1L8irFWvF9ZV0R1itVfZ0fzMiyMvzJCcnblK-HDCn31U";
    let new_title = "[TEST] Raport do faktury nr 4/2025";

    let new_doc_id = docs::copy_template(&drive, template_id, new_title)
        .await
        .map_err(|err| anyhow!("Failed to copy template document: {}", err))?;

    docs::fill_placeholders(&docs, &new_doc_id, activities)
        .await
        .map_err(|err| anyhow!("Failed to fill placeholders in document: {}", err))?;

    Ok(())
}
