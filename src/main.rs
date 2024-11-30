use chrono::naive::NaiveDate;
use chrono::{DateTime, Datelike, Local, Utc};
use clap::{arg, command, Parser};
use derive_new::new;
use futures::future;
use octocrab::models::pulls::PullRequest;
use octocrab::models::repos::RepoCommit;
use octocrab::{params, Octocrab, Page, Result};
use std::fmt::Display;

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
async fn main() -> octocrab::Result<()> {
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
        .build()?;

    let repos: Vec<String> = get_organization_repositories(&octocrab, &cli.owner).await?;

    use snafu::Backtrace;

    let activities = future::join_all(repos.into_iter().map(|repo| {
        let octocrab = octocrab.clone();
        let since = cli.since();
        let until = cli.until();
        let user = cli.user.clone();
        let owner = cli.owner.clone();
        tokio::task::spawn(async move {
            list_activity(&octocrab, &owner, &repo, user, since, until).await
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
    .collect::<octocrab::Result<Vec<_>>>()?;

    for activity in activities
        .iter()
        .filter(|activity| !activity.activities.is_empty())
    {
        print!("{}", activity);
    }

    Ok(())
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
        .per_page(100)
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
    owner: &String,
    repo: &String,
    author: &String,
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
        .per_page(100)
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
    owner: &String,
    repo: &String,
    commit: &RepoCommit,
) -> octocrab::Result<Vec<PullRequest>> {
    let mut result: Vec<PullRequest> = Vec::new();

    let stream = octocrab
        .repos(owner, repo)
        .list_pulls(commit.sha.to_string())
        .per_page(100)
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
    owner: String,
    repo: String,
    pull_number: &u64,
) -> octocrab::Result<Vec<RepoCommit>> {
    let mut result: Vec<RepoCommit> = Vec::new();
    let mut current_page = octocrab.list_commits(&owner, &repo, pull_number).await?;

    let mut commits = current_page.take_items();
    for commit in commits.drain(..) {
        result.push(commit)
    }

    Ok(result)
}

struct Activity {
    repository: String,
    activities: Vec<ActivityDetail>,
}

#[derive(PartialEq, new)]
struct PullRequestWithCommits {
    pull: PullRequest,
    commits: Vec<RepoCommit>,
    user: String,
}

#[derive(PartialEq)]
enum ActivityDetail {
    SingleCommit(Box<RepoCommit>),
    PullRequest(Box<PullRequestWithCommits>),
}

impl Display for Activity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Kontrybucja do repozytorium kodu \"{}\":",
            self.repository
        )?;

        for pull in &self.activities {
            write!(f, "{}", pull)?;
        }

        write!(f, "\n")
    }
}

impl Display for ActivityDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActivityDetail::SingleCommit(commit) => {
                writeln!(f, "{}: ({})", commit.commit.message, &commit.sha[0..7])
            }
            ActivityDetail::PullRequest(pr_with_commits) => {
                let formatted_commits: String = pr_with_commits
                    .commits
                    .iter()
                    .filter(|c| {
                        *c.author
                            .clone()
                            .map(|u| u.login == pr_with_commits.user)
                            .get_or_insert(false)
                    })
                    .map(|cp| &cp.sha[0..7])
                    .collect::<Vec<_>>()
                    .join(", ");

                let pull = pr_with_commits.pull.clone();

                let title = pull.title.unwrap_or(
                    pr_with_commits
                        .commits
                        .first()
                        .unwrap()
                        .commit
                        .message
                        .clone(),
                );
                writeln!(f, "#{}: {} ({})", pull.number, title, &formatted_commits)
            }
        }
    }
}

async fn list_activity(
    octocrab: &octocrab::Octocrab,
    owner: &String,
    repo: &String,
    user: String,
    since: NaiveDate,
    until: NaiveDate,
) -> octocrab::Result<Activity> {
    let mut result: Vec<ActivityDetail> = Vec::new();
    let repository = repo.to_string();

    let pull_commits = match list_user_commits(octocrab, &owner, repo, &user, since, until).await {
        Ok(commits) => commits,
        Err(e) => {
            eprintln!("Error fetching commits for {}/{}: {}", owner, &repo, e);
            Vec::new()
        }
    };

    for commit in pull_commits {
        let pulls = get_associated_pull_requests(octocrab, owner, repo, &commit).await?;

        if pulls.is_empty() {
            result.push(ActivityDetail::SingleCommit(Box::new(commit)));
        } else {
            for pull in pulls {
                let commits = get_pull_request_commits(
                    octocrab,
                    owner.to_string(),
                    repo.to_string(),
                    &pull.number,
                )
                .await?;
                result.push(ActivityDetail::PullRequest(Box::new(
                    PullRequestWithCommits::new(pull, commits, user.to_string()),
                )));
            }
        }
    }
    result.dedup();

    Ok(Activity {
        repository,
        activities: result,
    })
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
        let url = format!("/repos/{owner}/{repo}/pulls/{pull_number}/commits");
        self.get(url, None::<&()>).await
    }
}
