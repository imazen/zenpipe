//! Unified role- and format-keyed routing permissions and provider preference rules.
//!
//! [`Routing`] owns directional format allow-lists and per-([`RuleKey`]) provider
//! [`Rule`]s. Tighter child rules compose monotonically via [`Routing::merge`]:
//! allow-lists intersect, denials union, `allow_fallback` ANDs, and conflicting
//! `require` pins fail cleanly with [`RoutingError::ConflictingRequire`].

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt;

use zencodec::{ImageFormat, RegistrationId};

#[cfg(feature = "media")]
use zencodec_media::registry::ContainerId;
#[cfg(feature = "media")]
use zencodec_media::track::CodecId as MediaCodecId;

/// Role and target key addressed by a [`Rule`] in [`Routing`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RuleKey {
    /// Still or animated image decoding for a format.
    ImageDecode(ImageFormat),
    /// Still or animated image encoding for a format.
    ImageEncode(ImageFormat),
    /// Direct encoded-domain transform between a source and target format.
    EncodedTransform(ImageFormat, ImageFormat),
    /// Container demuxing for a media container.
    #[cfg(feature = "media")]
    Demux(ContainerId),
    /// Container muxing for a media container.
    #[cfg(feature = "media")]
    Mux(ContainerId),
    /// Compressed video bitstream decoding.
    #[cfg(feature = "media")]
    VideoDecode(MediaCodecId),
    /// Compressed video bitstream encoding.
    #[cfg(feature = "media")]
    VideoEncode(MediaCodecId),
    /// Color management provider selection.
    Cms,
}

/// Errors produced when composing or validating [`Routing`] rules.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RoutingError {
    /// A tighter scope requested a different required provider ID than its parent.
    ConflictingRequire {
        /// Rule key where the conflict occurred.
        key: RuleKey,
        /// Provider required by the parent ceiling.
        parent: RegistrationId,
        /// Provider required by the narrower request.
        child: RegistrationId,
    },
    /// A rule both required and denied the same provider ID.
    RequiredProviderDenied {
        /// Rule key where the conflict occurred.
        key: RuleKey,
        /// Provider ID that was both required and denied.
        id: RegistrationId,
    },
}

impl fmt::Display for RoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConflictingRequire { key, parent, child } => write!(
                f,
                "conflicting required provider for {key:?}: parent requires {parent}, child requires {child}"
            ),
            Self::RequiredProviderDenied { key, id } => {
                write!(f, "provider {id} is both required and denied for {key:?}")
            }
        }
    }
}

impl core::error::Error for RoutingError {}

/// Selection policy for a single [`RuleKey`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// Ordered preference list of provider IDs (highest priority first).
    pub prefer: Vec<RegistrationId>,
    /// Optional hard pin requiring this exact provider ID.
    pub require: Option<RegistrationId>,
    /// Explicitly denied provider IDs.
    pub deny: BTreeSet<RegistrationId>,
    /// Whether reversible preparation capability refusals may advance to the next candidate.
    pub allow_fallback: bool,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            prefer: Vec::new(),
            require: None,
            deny: BTreeSet::new(),
            allow_fallback: true,
        }
    }
}

impl Rule {
    /// Create a default permissive rule (no pin, no denials, fallback allowed).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set ordered provider preference IDs.
    #[must_use]
    pub fn with_prefer(mut self, prefer: impl IntoIterator<Item = RegistrationId>) -> Self {
        self.prefer = prefer.into_iter().collect();
        self
    }

    /// Pin a single required provider ID.
    #[must_use]
    pub fn with_require(mut self, id: RegistrationId) -> Self {
        self.require = Some(id);
        self
    }

    /// Deny a specific provider ID.
    #[must_use]
    pub fn denying(mut self, id: RegistrationId) -> Self {
        self.deny.insert(id);
        self
    }

    /// Control whether reversible preparation refusals may fall back to lower-ranked candidates.
    #[must_use]
    pub fn with_allow_fallback(mut self, allow: bool) -> Self {
        self.allow_fallback = allow;
        self
    }

    /// Validate internal consistency of this rule.
    pub fn validate(&self, key: &RuleKey) -> Result<(), RoutingError> {
        if let Some(req) = &self.require
            && self.deny.contains(req)
        {
            return Err(RoutingError::RequiredProviderDenied {
                key: key.clone(),
                id: req.clone(),
            });
        }
        Ok(())
    }

    /// Monotonically merge a narrower child rule under `self` (the parent ceiling).
    pub fn merge(&self, key: &RuleKey, child: &Self) -> Result<Self, RoutingError> {
        self.validate(key)?;
        child.validate(key)?;

        let require = match (&self.require, &child.require) {
            (Some(p), Some(c)) if p != c => {
                return Err(RoutingError::ConflictingRequire {
                    key: key.clone(),
                    parent: p.clone(),
                    child: c.clone(),
                });
            }
            (Some(p), _) => Some(p.clone()),
            (None, Some(c)) => Some(c.clone()),
            (None, None) => None,
        };

        let mut deny = self.deny.clone();
        deny.extend(child.deny.iter().cloned());

        if let Some(req) = &require
            && deny.contains(req)
        {
            return Err(RoutingError::RequiredProviderDenied {
                key: key.clone(),
                id: req.clone(),
            });
        }

        let mut prefer = Vec::with_capacity(child.prefer.len() + self.prefer.len());
        for id in child.prefer.iter().chain(self.prefer.iter()) {
            if !deny.contains(id) && !prefer.contains(id) {
                prefer.push(id.clone());
            }
        }

        Ok(Self {
            prefer,
            require,
            deny,
            allow_fallback: self.allow_fallback && child.allow_fallback,
        })
    }

    /// Check whether `id` is permitted by this rule.
    #[must_use]
    pub fn permits(&self, id: &RegistrationId) -> bool {
        if self.deny.contains(id) {
            return false;
        }
        match &self.require {
            Some(req) => req == id,
            None => true,
        }
    }

    /// Return the deterministic preference rank of `id` (`None` if unranked).
    #[must_use]
    pub fn preference_rank(&self, id: &RegistrationId) -> Option<usize> {
        self.prefer.iter().position(|p| p == id)
    }
}

fn push_unique<T: PartialEq>(vec: &mut Vec<T>, item: T) {
    if !vec.contains(&item) {
        vec.push(item);
    }
}

fn intersect_vec<T: Clone + PartialEq>(a: &Option<Vec<T>>, b: &Option<Vec<T>>) -> Option<Vec<T>> {
    match (a, b) {
        (Some(sa), Some(sb)) => Some(
            sa.iter()
                .filter(|x| sb.contains(x))
                .cloned()
                .collect(),
        ),
        (Some(sa), None) => Some(sa.clone()),
        (None, Some(sb)) => Some(sb.clone()),
        (None, None) => None,
    }
}

/// Directional format permissions and per-[`RuleKey`] provider rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Routing {
    input_formats: Option<Vec<ImageFormat>>,
    denied_input_formats: Vec<ImageFormat>,
    output_formats: Option<Vec<ImageFormat>>,
    denied_output_formats: Vec<ImageFormat>,
    #[cfg(feature = "media")]
    input_containers: Option<BTreeSet<ContainerId>>,
    #[cfg(feature = "media")]
    denied_input_containers: BTreeSet<ContainerId>,
    #[cfg(feature = "media")]
    output_containers: Option<BTreeSet<ContainerId>>,
    #[cfg(feature = "media")]
    denied_output_containers: BTreeSet<ContainerId>,
    rules: Vec<(RuleKey, Rule)>,
}

impl Routing {
    /// Create an unrestricted routing policy (all registered formats and providers permitted).
    #[must_use]
    pub fn unrestricted() -> Self {
        Self::default()
    }

    /// Restrict permitted input image formats to `formats`.
    #[must_use]
    pub fn with_input_formats(mut self, formats: impl IntoIterator<Item = ImageFormat>) -> Self {
        let mut list = Vec::new();
        for f in formats {
            push_unique(&mut list, f);
        }
        self.input_formats = Some(list);
        self
    }

    /// Deny an input image format.
    #[must_use]
    pub fn denying_input_format(mut self, format: ImageFormat) -> Self {
        push_unique(&mut self.denied_input_formats, format);
        self
    }

    /// Restrict permitted output image formats to `formats`.
    #[must_use]
    pub fn with_output_formats(mut self, formats: impl IntoIterator<Item = ImageFormat>) -> Self {
        let mut list = Vec::new();
        for f in formats {
            push_unique(&mut list, f);
        }
        self.output_formats = Some(list);
        self
    }

    /// Deny an output image format.
    #[must_use]
    pub fn denying_output_format(mut self, format: ImageFormat) -> Self {
        push_unique(&mut self.denied_output_formats, format);
        self
    }

    /// Restrict permitted input media containers.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn with_input_containers(
        mut self,
        containers: impl IntoIterator<Item = ContainerId>,
    ) -> Self {
        self.input_containers = Some(containers.into_iter().collect());
        self
    }

    /// Deny an input media container.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn denying_input_container(mut self, container: ContainerId) -> Self {
        self.denied_input_containers.insert(container);
        self
    }

    /// Restrict permitted output media containers.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn with_output_containers(
        mut self,
        containers: impl IntoIterator<Item = ContainerId>,
    ) -> Self {
        self.output_containers = Some(containers.into_iter().collect());
        self
    }

    /// Deny an output media container.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn denying_output_container(mut self, container: ContainerId) -> Self {
        self.denied_output_containers.insert(container);
        self
    }

    fn rule_mut(&mut self, key: &RuleKey) -> &mut Rule {
        if let Some(pos) = self.rules.iter().position(|(k, _)| k == key) {
            &mut self.rules[pos].1
        } else {
            self.rules.push((key.clone(), Rule::default()));
            &mut self.rules.last_mut().expect("pushed").1
        }
    }

    /// Attach or replace a [`Rule`] for `key`.
    pub fn with_rule(mut self, key: RuleKey, rule: Rule) -> Result<Self, RoutingError> {
        rule.validate(&key)?;
        if let Some(pos) = self.rules.iter().position(|(k, _)| k == &key) {
            self.rules[pos].1 = rule;
        } else {
            self.rules.push((key, rule));
        }
        Ok(self)
    }

    /// Convenience helper: set ordered preferences for `key`.
    #[must_use]
    pub fn preferring(
        mut self,
        key: RuleKey,
        ids: impl IntoIterator<Item = RegistrationId>,
    ) -> Self {
        let entry = self.rule_mut(&key);
        entry.prefer = ids.into_iter().collect();
        self
    }

    /// Convenience helper: deny `id` for `key`.
    pub fn denying(mut self, key: RuleKey, id: RegistrationId) -> Result<Self, RoutingError> {
        let entry = self.rule_mut(&key);
        entry.deny.insert(id);
        entry.validate(&key)?;
        Ok(self)
    }

    /// Convenience helper: require `id` for `key`.
    pub fn requiring(mut self, key: RuleKey, id: RegistrationId) -> Result<Self, RoutingError> {
        let entry = self.rule_mut(&key);
        entry.require = Some(id);
        entry.validate(&key)?;
        Ok(self)
    }

    /// Look up the effective [`Rule`] for `key`, if any.
    #[must_use]
    pub fn rule(&self, key: &RuleKey) -> Option<&Rule> {
        self.rules.iter().find(|(k, _)| k == key).map(|(_, r)| r)
    }

    /// Check whether `format` is permitted as a decoded input.
    #[must_use]
    pub fn permits_input_format(&self, format: ImageFormat) -> bool {
        if self.denied_input_formats.contains(&format) {
            return false;
        }
        match &self.input_formats {
            Some(allowed) => allowed.contains(&format),
            None => true,
        }
    }

    /// Check whether `format` is permitted as an encoded output.
    #[must_use]
    pub fn permits_output_format(&self, format: ImageFormat) -> bool {
        if self.denied_output_formats.contains(&format) {
            return false;
        }
        match &self.output_formats {
            Some(allowed) => allowed.contains(&format),
            None => true,
        }
    }

    /// Check whether `container` is permitted as a demuxed input.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn permits_input_container(&self, container: &ContainerId) -> bool {
        if self.denied_input_containers.contains(container) {
            return false;
        }
        match &self.input_containers {
            Some(allowed) => allowed.contains(container),
            None => true,
        }
    }

    /// Check whether `container` is permitted as a muxed output.
    #[cfg(feature = "media")]
    #[must_use]
    pub fn permits_output_container(&self, container: &ContainerId) -> bool {
        if self.denied_output_containers.contains(container) {
            return false;
        }
        match &self.output_containers {
            Some(allowed) => allowed.contains(container),
            None => true,
        }
    }

    /// Check whether a provider ID is permitted under `key`.
    #[must_use]
    pub fn permits_provider(&self, key: &RuleKey, id: &RegistrationId) -> bool {
        match self.rule(key) {
            Some(rule) => rule.permits(id),
            None => true,
        }
    }

    /// Whether fallback after a reversible capability refusal is permitted for `key`.
    #[must_use]
    pub fn allows_fallback(&self, key: &RuleKey) -> bool {
        self.rule(key).is_none_or(|r| r.allow_fallback)
    }

    /// Merge `self` (child request) under a trusted `ceiling` (`ceiling.merge(self)`).
    pub fn merge_under(&self, ceiling: &Self) -> Result<Self, RoutingError> {
        ceiling.merge(self)
    }

    /// Monotonically merge a narrower child `Routing` under `self` (the ceiling).
    pub fn merge(&self, child: &Self) -> Result<Self, RoutingError> {
        let mut denied_input_formats = self.denied_input_formats.clone();
        for &f in &child.denied_input_formats {
            push_unique(&mut denied_input_formats, f);
        }

        let mut denied_output_formats = self.denied_output_formats.clone();
        for &f in &child.denied_output_formats {
            push_unique(&mut denied_output_formats, f);
        }

        #[cfg(feature = "media")]
        let intersect_containers = |a: &Option<BTreeSet<ContainerId>>,
                                    b: &Option<BTreeSet<ContainerId>>|
         -> Option<BTreeSet<ContainerId>> {
            match (a, b) {
                (Some(sa), Some(sb)) => Some(sa.intersection(sb).cloned().collect()),
                (Some(sa), None) => Some(sa.clone()),
                (None, Some(sb)) => Some(sb.clone()),
                (None, None) => None,
            }
        };

        #[cfg(feature = "media")]
        let mut denied_input_containers = self.denied_input_containers.clone();
        #[cfg(feature = "media")]
        denied_input_containers.extend(child.denied_input_containers.iter().cloned());

        #[cfg(feature = "media")]
        let mut denied_output_containers = self.denied_output_containers.clone();
        #[cfg(feature = "media")]
        denied_output_containers.extend(child.denied_output_containers.iter().cloned());

        let mut rules = Vec::with_capacity(self.rules.len() + child.rules.len());
        for (k, parent_rule) in &self.rules {
            if let Some(child_rule) = child.rule(k) {
                rules.push((k.clone(), parent_rule.merge(k, child_rule)?));
            } else {
                parent_rule.validate(k)?;
                rules.push((k.clone(), parent_rule.clone()));
            }
        }
        for (k, child_rule) in &child.rules {
            if !rules.iter().any(|(ek, _)| ek == k) {
                child_rule.validate(k)?;
                rules.push((k.clone(), child_rule.clone()));
            }
        }

        Ok(Self {
            input_formats: intersect_vec(&self.input_formats, &child.input_formats),
            denied_input_formats,
            output_formats: intersect_vec(&self.output_formats, &child.output_formats),
            denied_output_formats,
            #[cfg(feature = "media")]
            input_containers: intersect_containers(&self.input_containers, &child.input_containers),
            #[cfg(feature = "media")]
            denied_input_containers,
            #[cfg(feature = "media")]
            output_containers: intersect_containers(
                &self.output_containers,
                &child.output_containers,
            ),
            #[cfg(feature = "media")]
            denied_output_containers,
            rules,
        })
    }

    /// Filter and deterministically rank candidate items for `key`.
    pub fn rank_candidates<T>(
        &self,
        key: &RuleKey,
        candidates: impl IntoIterator<Item = (RegistrationId, T)>,
    ) -> Vec<(RegistrationId, T)> {
        let rule = self.rule(key);
        let mut indexed: Vec<(usize, Option<usize>, RegistrationId, T)> = candidates
            .into_iter()
            .enumerate()
            .filter(|(_, (id, _))| rule.is_none_or(|r| r.permits(id)))
            .map(|(orig_idx, (id, item))| {
                let pref = rule.and_then(|r| r.preference_rank(&id));
                (orig_idx, pref, id, item)
            })
            .collect();

        indexed.sort_by(|a, b| {
            let key_a = (a.1.unwrap_or(usize::MAX), a.0, a.2.as_str());
            let key_b = (b.1.unwrap_or(usize::MAX), b.0, b.2.as_str());
            key_a.cmp(&key_b)
        });

        let allow_fallback = rule.is_none_or(|r| r.allow_fallback);
        if !allow_fallback && indexed.len() > 1 {
            indexed.truncate(1);
        }

        indexed
            .into_iter()
            .map(|(_, _, id, item)| (id, item))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn rid(s: &str) -> RegistrationId {
        RegistrationId::new(s).unwrap()
    }

    #[test]
    fn routing_merge_intersects_allows_unions_denies_and_rejects_conflicts() {
        let parent = Routing::unrestricted()
            .with_output_formats([ImageFormat::Jpeg, ImageFormat::WebP, ImageFormat::Png])
            .denying_output_format(ImageFormat::Png)
            .with_rule(
                RuleKey::ImageEncode(ImageFormat::Jpeg),
                Rule::new()
                    .with_prefer([rid("org.imazen.jpeg.zenjpeg"), rid("org.imazen.jpeg.mozjpeg")])
                    .denying(rid("org.imazen.jpeg.legacy")),
            )
            .unwrap();

        let child = Routing::unrestricted()
            .with_output_formats([ImageFormat::Jpeg, ImageFormat::Avif])
            .with_rule(
                RuleKey::ImageEncode(ImageFormat::Jpeg),
                Rule::new()
                    .with_prefer([rid("org.imazen.jpeg.mozjpeg")])
                    .with_allow_fallback(false),
            )
            .unwrap();

        let merged = parent.merge(&child).unwrap();
        assert!(merged.permits_output_format(ImageFormat::Jpeg));
        assert!(!merged.permits_output_format(ImageFormat::WebP));
        assert!(!merged.permits_output_format(ImageFormat::Avif));
        assert!(!merged.permits_output_format(ImageFormat::Png));
        assert!(!merged.allows_fallback(&RuleKey::ImageEncode(ImageFormat::Jpeg)));

        let ranked = merged.rank_candidates(
            &RuleKey::ImageEncode(ImageFormat::Jpeg),
            vec![
                (rid("org.imazen.jpeg.zenjpeg"), "zen"),
                (rid("org.imazen.jpeg.mozjpeg"), "moz"),
                (rid("org.imazen.jpeg.legacy"), "legacy"),
            ],
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].1, "moz");

        let p_req = Routing::unrestricted()
            .requiring(
                RuleKey::ImageEncode(ImageFormat::Jpeg),
                rid("org.imazen.jpeg.zenjpeg"),
            )
            .unwrap();
        let c_req = Routing::unrestricted()
            .requiring(
                RuleKey::ImageEncode(ImageFormat::Jpeg),
                rid("org.imazen.jpeg.mozjpeg"),
            )
            .unwrap();
        assert!(matches!(
            p_req.merge(&c_req),
            Err(RoutingError::ConflictingRequire { .. })
        ));
    }
}
