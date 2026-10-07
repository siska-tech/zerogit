//! Creating, deleting and listing tags.

use crate::commit::cleanup_message;
use crate::error::Result;
use crate::objects::{ObjectType, Oid, Signature, TagObject};
use crate::refs::Tag;
use crate::repository::{validate_ref_name, Repository};

impl Repository {
    /// Creates a lightweight tag: `refs/tags/<name>` pointing directly at an
    /// object.
    ///
    /// # Arguments
    ///
    /// * `name` - The tag name (without `refs/tags/` prefix).
    /// * `target` - The object to tag (any type). If `None`, the HEAD commit.
    ///
    /// # Errors
    ///
    /// - `Error::InvalidRefName` if the name is not a valid tag name.
    /// - `Error::RefAlreadyExists` if the tag (or a conflicting `a`/`a/b`
    ///   tag) exists. Existing tags are never overwritten; delete first.
    /// - `Error::ObjectNotFound` if the target does not exist.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.create_tag("v1.0.0", None).unwrap();
    /// ```
    pub fn create_tag(&self, name: &str, target: Option<Oid>) -> Result<Tag> {
        let (ref_name, target_oid, _) = self.prepare_tag(name, target)?;
        self.write_ref(&ref_name, &target_oid)?;
        Ok(Tag::lightweight(name, target_oid))
    }

    /// Creates an annotated tag: a tag object recording the target, the
    /// tagger and a message, and `refs/tags/<name>` pointing to it.
    ///
    /// The tagger is given like the author of [`Repository::create_commit`],
    /// with the current time. The message is cleaned up as `git tag -m`
    /// does (trailing whitespace and surrounding blank lines removed, ending
    /// with a newline).
    ///
    /// # Arguments
    ///
    /// * `name` - The tag name (without `refs/tags/` prefix).
    /// * `target` - The object to tag (any type; the tag object records its
    ///   actual type). If `None`, the HEAD commit.
    /// * `message` - The tag message.
    /// * `tagger_name` - The tagger's name.
    /// * `tagger_email` - The tagger's email.
    ///
    /// # Errors
    ///
    /// The same as [`Repository::create_tag`]. Nothing is written on error.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// let tag = repo
    ///     .create_annotated_tag("v1.0.0", None, "Release 1.0.0", "John Doe", "john@example.com")
    ///     .unwrap();
    /// assert!(tag.is_annotated());
    /// ```
    pub fn create_annotated_tag(
        &self,
        name: &str,
        target: Option<Oid>,
        message: &str,
        tagger_name: &str,
        tagger_email: &str,
    ) -> Result<Tag> {
        let (ref_name, target_oid, target_type) = self.prepare_tag(name, target)?;
        let tagger = Signature::now(tagger_name, tagger_email);
        let message = cleanup_message(message);
        let content = format!(
            "object {}\ntype {}\ntag {}\ntagger {}\n\n{}",
            target_oid.to_hex(),
            target_type.as_str(),
            name,
            tagger.to_git_string(),
            message
        );
        let tag_oid = self
            .object_store()
            .write(ObjectType::Tag, content.as_bytes())?;
        self.write_ref(&ref_name, &tag_oid)?;
        let tag_obj = TagObject::parse(self.object_store().read(&tag_oid)?)?;
        Ok(Tag::annotated(
            name,
            *tag_obj.object(),
            tag_obj.message().to_string(),
            tag_obj.tagger().clone(),
        ))
    }

    /// Validates a new tag and resolves its target and the target's type.
    fn prepare_tag(&self, name: &str, target: Option<Oid>) -> Result<(String, Oid, ObjectType)> {
        validate_ref_name("tag", name)?;
        let target_oid = match target {
            Some(oid) => oid,
            None => *self.head()?.oid(),
        };
        let target_type = self.object_store().read(&target_oid)?.object_type;
        let ref_name = self.check_new_ref("tags", name)?;
        Ok((ref_name, target_oid, target_type))
    }

    /// Deletes a tag (lightweight or annotated). A tag object is left in the
    /// object database, as Git leaves it.
    ///
    /// # Errors
    ///
    /// - `Error::RefNotFound` if the tag does not exist.
    /// - `Error::Locked` if the tag or `packed-refs` is locked.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    /// repo.delete_tag("v1.0.0").unwrap();
    /// ```
    pub fn delete_tag(&self, name: &str) -> Result<()> {
        validate_ref_name("tag", name)?;
        let ref_name = format!("refs/tags/{}", name);
        self.delete_ref(&ref_name, "refs/tags")
    }

    /// Lists all tags in the repository.
    ///
    /// Returns a vector of `Tag` objects representing all tags in `refs/tags/`.
    /// For annotated tags, the message and tagger information are included.
    ///
    /// # Returns
    ///
    /// A vector of tags, sorted by name.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use zerogit::repository::Repository;
    ///
    /// let repo = Repository::open("path/to/repo").unwrap();
    ///
    /// for tag in repo.tags().unwrap() {
    ///     println!("{} -> {}", tag.name(), tag.target().short());
    ///     if let Some(message) = tag.message() {
    ///         println!("  {}", message);
    ///     }
    /// }
    /// ```
    pub fn tags(&self) -> Result<Vec<Tag>> {
        let object_store = self.object_store();
        let mut result = Vec::new();
        for resolved in self.ref_store().resolved_refs("refs/tags/")? {
            let name = resolved.name.strip_prefix("refs/tags/").unwrap();
            let raw = object_store.read(&resolved.oid)?;
            if raw.object_type == ObjectType::Tag {
                let tag_obj = TagObject::parse(raw)?;
                result.push(Tag::annotated(
                    name,
                    *tag_obj.object(),
                    tag_obj.message().to_string(),
                    tag_obj.tagger().clone(),
                ));
            } else {
                result.push(Tag::lightweight(name, resolved.oid));
            }
        }
        Ok(result)
    }
}
