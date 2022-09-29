use std::collections::HashMap;
use clap::{command, Command, Parser};
use std::env::var;
use github_rs::client::{Github, Executor};
use iso8601_timestamp::Timestamp;
use serde_derive::Deserialize;
use rayon::prelude::*;

#[derive(Deserialize, Debug)]
struct Repo {
//    archived: bool,
    name: String
}

#[derive(Deserialize)]
struct User {
   login: String
}

#[derive(Deserialize)]
struct PR {
    id: u32,
    number: u32,
    user: User,
    created_at: Timestamp,
    updated_at: Timestamp
}

#[derive(Deserialize)]
struct CommitSHA {
    sha: String,
}

#[derive(Deserialize)]
struct Pull {
    url: String,
    id: u32,
    number: u32,
    state: String,
    title: String,
    user: User,
    created_at: Timestamp,
    updated_at: Timestamp,
    closed_at: Option<Timestamp>,
    merged_at: Option<Timestamp>,
    merge_commit_sha: String,
    assignee: Option<User>,
    requested_reviewers: Vec<User>,
}

#[derive(Deserialize)]
struct PullCommit {
    sha: String,
    pull: Pull
}


fn main() {
    let owner = "restaumatic";
    let user = "jborkowski";

    let gh_token = std::env::var("GH_TOKEN").expect("GH_TOKEN must be set.");
    let clientg = Github::new(gh_token).expect("Failed to create Github client");
    let since = Timestamp::parse("2022-08-01T00:00:00Z").expect("Failed to parse 'since' date");
    let until = Timestamp::parse("2022-09-30T23:59:59Z").expect("Failed to parse 'until' date");


    let results: Vec<(String, HashMap<u32, Vec<PullCommit>>)> = organization_repositories(&clientg, owner)
	.par_iter()
	.map(|repo| {
             var("GH_TOKEN")
                .map_err(|e| "GH_TOKEN must be set.")
		.and_then(|token| Github::new(token).map_err(|e| "Failed to create Github client"))
		.and_then(|client| Ok(activity_for(&client, &owner, &repo.name, &user, &since, &until)))
		.map(|activity|	(repo.name.clone(), activity))
		.expect("Failed to parse 'until' date")
	})
	.collect::<Vec<_>>();

    println!("{}", print_result(&results))
}



fn print_result(results: &Vec<(String, HashMap<u32, Vec<PullCommit>>)>) -> String {
    let mut out = String::new();
    out.push_str("Contributions:\n");

    results
	.into_iter()
	.for_each(|(project, pulls)| {
	    if !pulls.is_empty() {
		let repo = format!("* Repository: {}\n", project);
		out.push_str(&repo);

		pulls.iter().for_each(|(pull, commits)| {
		    commits.first().iter().for_each(|f| {
			let pull = format!("** #{}: {} ({})\n", pull, f.pull.title, print_commits(&commits));
			out.push_str(&pull);
		    })
		});
	    };
	});

    out
}

fn print_commits(commits: &Vec<PullCommit>) -> String {
    commits.into_iter().map(|cp| &cp.sha[0..6]).collect::<Vec<_>>().join(", ")
}

fn organization_repositories(client: &Github, owner: &str) -> Vec<Repo> {
    let mut result: Vec<Repo> = Vec::new();


    fn fetch_page(client: &Github, owner: &str, page: i8) -> Option<Vec<Repo>> {
	let repos_endpoint = format!(
	    "orgs/{}/repos?type=all&page={}&per_page=100",
            owner, page
	);

	client
	    .get()
	    .custom_endpoint(&repos_endpoint)
	    .execute::<Vec<Repo>>()
	    .expect("Cannot fetch organisation repos")
	    .2
    }


    for i in 1..=5 {
	match fetch_page(&client, &owner, i) {
	    Some(mut vec) => result.append(&mut vec),
	    None => break,
	}
    }

    println!("Organization have {} repositories.", result.len());

    result

}

fn activity_for(client: &Github, owner: &str, repo_name: &str, user_login: &str, since: &Timestamp, until: &Timestamp) -> HashMap<u32, Vec<PullCommit>> {
    let mut results: HashMap<u32, Vec<PullCommit>> = HashMap::new();

    let commits_endpoint = format!(
	"repos/{}/{}/commits?author={}&since={}&until={}&per_page=100",
        owner, repo_name, user_login, since.format(), until.format()
    );

    client
        .get()
	.custom_endpoint(&commits_endpoint)
        .execute::<Vec<CommitSHA>>()
	.expect("Cannot fetch pulls")
	.2.unwrap_or(Vec::new())
	.iter()
	.for_each(|commit_sha| {
 	    let sha = commit_sha.sha.clone();
	    let details_endpoint = format!(
		"repos/{}/{}/commits/{}/pulls",
		owner, repo_name, sha
	    );

	    let res = client
		.get()
		.custom_endpoint(&details_endpoint)
		.execute::<Vec<Pull>>();

	match res {
	    Ok((_,_,Some(vec))) => {
		vec.into_iter().for_each(|pull| {
		    if results.contains_key(&pull.number) {
			if let Some(old) = results.get_mut(&pull.number) {
			    let sha = commit_sha.sha.clone();
			    old.push(PullCommit { pull, sha })
			}
		    } else {
			let sha = commit_sha.sha.clone();
			results.insert(pull.number, vec![PullCommit { pull, sha }]);
		    }
		})
	    },
	    Ok(_) =>
		(),
	    Err(err) =>
		println!("{:?}", err)

	}


    });

	results

}
