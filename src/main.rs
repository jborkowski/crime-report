use chrono::naive::NaiveDate;
use chrono::{DateTime, Datelike, Local, Utc};
use clap::{arg, command, Parser};
use octocrab::models::pulls::PullRequest;
use octocrab::models::repos::RepoCommit;
use octocrab::{params, Octocrab, Page, Result};
use std::collections::HashMap;
use tokio::sync::mpsc;

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

#[derive(Debug)]
enum Command {
    Set { key: String, val: Vec<String> },
}

#[tokio::main]
async fn main() -> octocrab::Result<()> {
    let cli = Cli::parse();

    let gh_token = match cli.gh_token {
        None => {
            match std::env::var("GH_TOKEN") {
                Ok(gh_token) => gh_token,
                Err(_) => {
                    eprintln!("GH_TOKEN is not provided as a command-line argument or environment variable");
                    std::process::exit(1);
                }
            }
        }
        Some(gh_token) => gh_token,
    };

    let since = NaiveDate::from_ymd_opt(cli.year, cli.month, 1).unwrap();

    let until = NaiveDate::from_ymd_opt(cli.year, cli.month + 1, 1).unwrap();

    println!(
        "Fetching activities for user: '{}' in '{}' organization ({} - {})\n",
        cli.user, cli.owner, since, until
    );

    let octocrab = octocrab::Octocrab::builder()
        .personal_token(gh_token)
        .build()?;

    let (tx, mut rx) = mpsc::channel(100);

    let repos: Vec<String> = get_organization_repositories(&octocrab, &cli.owner).await?;

    let total_repos = &repos.len();

    for repo in repos {
        let tx = tx.clone();
        let octocrab = octocrab.clone();
        let owner = cli.owner.clone();
        let user = cli.user.clone();

        tokio::spawn(async move {
            let activities = list_activity(&octocrab, &owner, &repo, &user, since, until)
                .await
                .unwrap();

            let cmd = Command::Set {
                key: repo,
                val: activities,
            };
            tx.send(cmd).await.unwrap();
        });
    }

    let mut results: HashMap<String, Vec<String>> = HashMap::new();

    while let Some(cmd) = rx.recv().await {
        match cmd {
            Command::Set { key, val } => {
                results.insert(key, val);
                if results.len() == *total_repos {
                    break;
                }
            }
        }
    }

    println!("{}", print_result(&results));

    Ok(())
}

fn print_result(results: &HashMap<String, Vec<String>>) -> String {
    let mut out = String::new();
    results.iter().for_each(|(repo, pulls)| {
        if !pulls.is_empty() {
            let repo = format!("Kontrybucja do repozytorium kodu \"{}\":\n", repo);
            out.push_str(&repo);

            pulls.iter().for_each(|pull| {
                out.push_str(pull);
            });

            out.push('\n');
        };
    });

    out
}

async fn get_organization_repositories(
    octocrab: &octocrab::Octocrab,
    owner: &String,
) -> octocrab::Result<Vec<String>> {
    let mut result = Vec::new();

    let stream = octocrab
        .orgs(owner)
        .list_repos()
        .sort(params::repos::Sort::Pushed)
        .direction(params::Direction::Descending)
        .send()
        .await?
        .into_stream(octocrab);

    tokio::pin!(stream);
    use futures_util::TryStreamExt;

    while let Some(repo) = stream.try_next().await? {
        result.push(repo.name);
    }

    Ok(result)
}

async fn list_user_commits(
    crab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    author: &str,
    since: NaiveDate,
    until: NaiveDate,
) -> octocrab::Result<Vec<RepoCommit>> {
    let mut result: Vec<RepoCommit> = Vec::new();

    let stream = crab
        .repos(owner, repo)
        .list_commits()
        .author(author)
        .since(DateTime::from_naive_utc_and_offset(since.into(), Utc))
        .until(DateTime::from_naive_utc_and_offset(until.into(), Utc))
        .send()
        .await?
        .into_stream(crab);

    tokio::pin!(stream);
    use futures_util::TryStreamExt;

    while let Some(commit) = stream.try_next().await? {
        result.push(commit);
    }

    Ok(result)
}

async fn get_associated_pull_requests(
    octocrab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    commit: &RepoCommit,
) -> octocrab::Result<Vec<PullRequest>> {
    let mut result: Vec<PullRequest> = Vec::new();

    let stream = octocrab
        .repos(owner, repo)
        .list_pulls(commit.sha.to_string())
        .send()
        .await?
        .into_stream(octocrab);

    tokio::pin!(stream);
    use futures_util::TryStreamExt;

    while let Some(pr) = stream.try_next().await? {
        result.push(pr);
    }

    Ok(result)
}

async fn get_pull_request_commits(
    octocrab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    pull_number: &u64,
) -> octocrab::Result<Vec<RepoCommit>> {
    let mut result: Vec<RepoCommit> = Vec::new();
    let mut current_page = octocrab.list_commits(owner, repo, pull_number).await?;

    let mut commits = current_page.take_items();
    for commit in commits.drain(..) {
        result.push(commit)
    }

    Ok(result)
}

async fn list_activity(
    octocrab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    user: &str,
    since: NaiveDate,
    until: NaiveDate,
) -> octocrab::Result<Vec<String>> {
    let mut result: Vec<String> = Vec::new();

    let pull_commits = match list_user_commits(octocrab, owner, repo, user, since, until).await {
        Ok(commits) => commits,
        Err(e) => {
            eprintln!("Error fetching commits for {}/{}: {}", owner, repo, e);
            Vec::new()
        }
    };

    for commit in pull_commits {
        let pulls = get_associated_pull_requests(octocrab, owner, repo, &commit).await?;

        if pulls.is_empty() {
            let commit_text: String =
                format!("{}: ({})\n", commit.commit.message, &commit.sha[0..7]);
            result.push(commit_text);
        } else {
            for pull in pulls {
                let commits = get_pull_request_commits(octocrab, owner, repo, &pull.number)
                    .await
                    .unwrap();
                let formatted_commits: String = commits
                    .iter()
                    .filter(|c| {
                        *c.author
                            .clone()
                            .map(|u| u.login == user)
                            .get_or_insert(false)
                    })
                    .map(|cp| &cp.sha[0..7])
                    .collect::<Vec<_>>()
                    .join(", ");

                let title = pull.title.unwrap_or(commit.commit.message.clone());
                let pull_text = format!("#{}: {} ({})\n", pull.number, title, &formatted_commits);
                result.push(pull_text);
            }
        }
    }
    result.dedup();

    Ok(result)
}

#[async_trait::async_trait]
trait PullsExt {
    async fn list_commits(
        &self,
        owner: &str,
        repo: &str,
        pull_number: &u64,
    ) -> Result<Page<RepoCommit>>;
}

#[async_trait::async_trait]
impl PullsExt for Octocrab {
    async fn list_commits(
        &self,
        owner: &str,
        repo: &str,
        pull_number: &u64,
    ) -> Result<Page<RepoCommit>> {
        let url = format!(
            "/repos/{owner}/{repo}/pulls/{pull_number}/commits",
            owner = owner,
            repo = repo,
            pull_number = pull_number
        );
        self.get(url, None::<&()>).await
    }
}
