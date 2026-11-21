//! Tags (issue #40): create lightweight or annotated (optionally signed) tags, delete them and
//! put a deleted one back exactly (by the object its ref pointed to).

use crate::branch::NameProblem;
use crate::{Cancel, GitError, Oid, Repository, Written};

impl Repository {
    fn tag_name_ok(&self, name: &str) -> Result<(), GitError> {
        let problem = if name.is_empty() {
            Some(NameProblem::Empty)
        } else if name.starts_with('-') {
            Some(NameProblem::LeadingDash)
        } else if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            Some(NameProblem::Whitespace)
        } else {
            let full = format!("refs/tags/{name}");
            match self.read(["check-ref-format", full.as_str()]) {
                Ok(_) => None,
                Err(GitError::Failed { code: Some(1), .. }) => Some(NameProblem::Rules),
                Err(e) => return Err(e),
            }
        };
        match problem {
            Some(problem) => Err(GitError::InvalidName {
                name: name.to_owned(),
                problem,
            }),
            None => Ok(()),
        }
    }

    /// Creates tag `name` at `target`. With a `message` the tag is annotated (message on stdin)
    /// and, with `sign`, signed as configured; without one it is lightweight.
    pub fn create_tag(
        &self,
        name: &str,
        target: &str,
        message: Option<&str>,
        sign: bool,
        cancel: &Cancel,
    ) -> Result<Written, GitError> {
        self.tag_name_ok(name)?;
        match message {
            None => self.write(["tag", "--end-of-options", name, target], None, cancel),
            Some(message) => {
                let mut args = vec!["tag", if sign { "-s" } else { "-a" }];
                args.extend([
                    "--cleanup=whitespace",
                    "-F",
                    "-",
                    "--end-of-options",
                    name,
                    target,
                ]);
                self.write(args, Some(message.as_bytes()), cancel)
            }
        }
    }

    /// Deletes a tag; returns the object its ref pointed to (the tag object of an annotated
    /// tag), which [`Repository::restore_tag`] puts back.
    pub fn delete_tag(&self, name: &str, cancel: &Cancel) -> Result<Oid, GitError> {
        self.tag_name_ok(name)?;
        let full = format!("refs/tags/{name}");
        let out = self.read([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            full.as_str(),
        ])?;
        let text = String::from_utf8_lossy(&out);
        let object = Oid::parse(text.trim())
            .ok_or_else(|| GitError::Parse(format!("rev-parse: {}", text.trim())))?;
        self.write(["tag", "-d", "--end-of-options", name], None, cancel)?;
        Ok(object)
    }

    /// Puts a deleted tag back as it was; refuses when a tag of that name exists again.
    pub fn restore_tag(&self, name: &str, object: &Oid, cancel: &Cancel) -> Result<(), GitError> {
        self.tag_name_ok(name)?;
        let full = format!("refs/tags/{name}");
        // The empty old value makes git refuse to overwrite an existing ref.
        self.write(
            ["update-ref", full.as_str(), object.as_str(), ""],
            None,
            cancel,
        )?;
        Ok(())
    }
}
