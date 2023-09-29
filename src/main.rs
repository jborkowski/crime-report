use std::collections::HashMap;
use octocrab::models::pulls::PullRequest;
use octocrab::models::repos::RepoCommit;
use clap::{Parser, command, arg};
use chrono::naive::NaiveDate;
use chrono::{DateTime, Datelike, Local, Utc};
use chrono::TimeZone;
use octocrab::{Octocrab, Page, Result, params};
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(next_line_help = true)]
struct Cli {
    #[arg(long)]
    #[arg(short = 's')]
    #[arg(default_value_t = default_since())]
    since: NaiveDate,
    #[arg(long)]
    #[arg(short = 'u')]
    #[arg(default_value_t = default_until())]
    until: NaiveDate,

    #[arg(long, default_value_t = String::from("restaumatic"))]
    owner: String,

    #[arg(long, short = 'U')]
    user: String,

    #[arg(long, default_value_t = std::env::var("GH_TOKEN").expect("GH_TOKEN must be set"))]
    gh_token: String
}

fn default_since() -> NaiveDate {
    let local: DateTime<Local> = Local::now();
    NaiveDate::from_ymd_opt(local.year(), local.month(), 1).unwrap()
}

fn default_until() -> NaiveDate {
    let local: DateTime<Local> = Local::now();
    NaiveDate::from_ymd_opt(local.year(), local.month(), local.day()).unwrap()
}

#[derive(Debug)]
enum Command {
    Set {
        key: String,
        val: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> octocrab::Result<()> {

    let cli = Cli::parse();

    println!("Fetching activities for user: '{}' in '{}' organization ({} - {})\n", cli.user, cli.owner, cli.since, cli.until);

    let since = Utc.from_utc_datetime(&cli.since.and_hms_opt(0,0,0).unwrap());
    let until = Utc.from_utc_datetime(&cli.until.and_hms_opt(0,0,0).unwrap());

    let octocrab = octocrab::Octocrab::builder()
        .personal_token(cli.gh_token)
        .build()?;

    let (tx, mut rx) = mpsc::channel(100);

    let repos: Vec<String> = get_organization_repositories(&octocrab, &cli.owner).await.unwrap();

    let total_repos = &repos.len();

    for repo in repos {
	      let tx = tx.clone();
	      let octocrab = octocrab.clone();
	      let owner = cli.owner.clone();
	      let user = cli.user.clone();

	      tokio::spawn(async move {
            let activities = list_activity(&octocrab, &owner, &repo, &user, since, until).await.unwrap();
	          let cmd = Command::Set {
		            key: repo,
		            val: activities
	          };
            tx.send(cmd).await.unwrap();
	      });
    }

    let mut results: HashMap<String, Vec<String>> = HashMap::new();

    while let Some(cmd) = rx.recv().await {
	      match cmd {
	          Command::Set {key, val} => {
		            results.insert(key, val);
		            if results.len() == total_repos.clone() {
		                break;
		            }
	          }
	      }
    };

    println!("{}", print_result(&results));

    Ok(())

}


fn print_result(results: &HashMap<String, Vec<String>>) -> String {
    let mut out = String::new();
    results
	.into_iter()
	.for_each(|(repo, pulls)| {
	    if !pulls.is_empty() {
		let repo = format!("Kontrybucja do repozytorium kodu \"{}\":\n", repo);
		out.push_str(&repo);

		pulls.iter().for_each(|pull| {
		    out.push_str(&pull);
		});

		out.push_str("\n");
	    };
	});

    out
}


async fn get_organization_repositories(octocrab: &octocrab::Octocrab, owner: &String) -> octocrab::Result<Vec<String>> {
    let mut result = Vec::new();

    let mut current_page = octocrab
	      .orgs(owner)
	      .list_repos()
	      .sort(params::repos::Sort::Pushed)
        .direction(params::Direction::Descending)
        .per_page(100)
        .send()
        .await?;

    let mut repos = current_page.take_items();
    for repo in repos.drain(..) {
	      result.push(repo.name)
    };

    while let Ok(Some(mut new_page)) = octocrab.get_page(&current_page.next).await {
        repos.extend(new_page.take_items());

        for repo in repos.drain(..) {
	          result.push(repo.name)
        }

        current_page = new_page;
    }

    Ok(result)
}

async fn list_user_commits(octocrab: &octocrab::Octocrab, owner: &str, repo: &str, author: &str, since: DateTime<Utc>, until: DateTime<Utc>) -> octocrab::Result<Vec<RepoCommit>> {
    let mut result: Vec<RepoCommit> = Vec::new();

    let mut current_page = octocrab
	      .repos(owner, repo)
	      .list_commits()
	      .author(author)
	      .since(since)
	      .until(until)
	      .send()
	      .await?;

    let mut commits = current_page.take_items();

    for commit in commits.drain(..) {
	      result.push(commit)
    };

    while let Ok(Some(mut new_page)) = octocrab.get_page(&current_page.next).await {
        commits.extend(new_page.take_items());

        for commit in commits.drain(..) {
	          result.push(commit)
        }

        current_page = new_page;
    }

    Ok(result)

}

async fn get_associated_pull_requests(octocrab: &octocrab::Octocrab, owner: &str, repo: &str, commit: &RepoCommit) -> octocrab::Result<Vec<PullRequest>>{
    let mut result: Vec<PullRequest> = Vec::new();
    let mut current_page = octocrab
	      .repos(owner, repo)
	      .list_pulls(commit.sha.to_string())
	      .send()
	      .await?;

    let mut prs = current_page.take_items();
    for pr in prs.drain(..) {
	      result.push(pr)
    };

    Ok(result)

}

async fn get_pull_request_commits(octocrab: &octocrab::Octocrab, owner: &str, repo: &str, pull_number: &u64) -> octocrab::Result<Vec<RepoCommit>>{
    let mut result: Vec<RepoCommit> = Vec::new();
    let mut current_page = octocrab
        .list_commits(owner, repo, pull_number)
	      .await?;

    let mut commits = current_page.take_items();
    for commit in commits.drain(..) {
	      result.push(commit)
    };

    Ok(result)
}

#[feature(option_result_contains)]
async fn list_activity(octocrab: &octocrab::Octocrab, owner: &str, repo: &str, user: &str, since: DateTime<Utc>, until: DateTime<Utc>) -> octocrab::Result<Vec<String>> {
    let mut result: Vec<String> = Vec::new();

    let pull_commits = list_user_commits(&octocrab, owner, repo, user, since, until).await.unwrap();

    for commit in pull_commits {
	      let pulls = get_associated_pull_requests(&octocrab, owner, repo, &commit).await.unwrap();

	      if pulls.is_empty() {
	          let commit_text: String = format!("{}: ({})\n", commit.commit.message, &commit.sha[0..7]);
	          result.push(commit_text);
	      } else {
	          for pull in pulls {
		            let commits = get_pull_request_commits(&octocrab, owner, repo, &pull.number).await.unwrap();
		            let formatted_commits: String =
                    commits
                        .iter()
                        .filter(|c| *c.author.clone().map(|u| u.login == user).get_or_insert(false))
                        .map(|cp| &cp.sha[0..7]).collect::<Vec<_>>().join(", ");
                
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
    async fn list_commits(&self, owner: &str, repo: &str, pull_number: &u64) -> Result<Page<RepoCommit>>;
}

#[async_trait::async_trait]
impl PullsExt for Octocrab {
    async fn list_commits(&self, owner: &str, repo: &str, pull_number: &u64) -> Result<Page<RepoCommit>> {
	      let url = format!(
	          "/repos/{owner}/{repo}/pulls/{pull_number}/commits",
            owner = owner,
            repo = repo,
	          pull_number = pull_number
	      );
        self.get(url, None::<&()>).await
    }
}
