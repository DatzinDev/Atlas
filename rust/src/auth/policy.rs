use crate::domain::{Dataset, Error, Principal};

fn has_role(principal: &Principal, allowed: &[String]) -> bool {
    principal.sub.as_ref().is_some_and(|s| !s.trim().is_empty())
        && principal.roles.iter().any(|role| allowed.contains(role))
}

pub fn view(dataset: &Dataset, principal: &Principal) -> Result<(), Error> {
    if dataset.enabled && (dataset.access.public || has_role(principal, &dataset.access.view_roles))
    {
        Ok(())
    } else {
        Err(Error::DatasetNotFound)
    }
}

pub fn query(dataset: &Dataset, principal: &Principal) -> Result<(), Error> {
    view(dataset, principal)?;
    if dataset.access.public || has_role(principal, &dataset.access.query_roles) {
        Ok(())
    } else {
        Err(Error::PermissionDenied)
    }
}

pub fn download(dataset: &Dataset, principal: &Principal) -> Result<(), Error> {
    view(dataset, principal)?;
    if (dataset.access.public && dataset.access.public_download)
        || has_role(principal, &dataset.access.download_roles)
    {
        Ok(())
    } else {
        Err(Error::PermissionDenied)
    }
}

pub fn admin(principal: &Principal) -> Result<(), Error> {
    if principal.sub.as_ref().is_some_and(|s| !s.trim().is_empty())
        && principal.roles.iter().any(|role| role == "atlas-admin")
    {
        Ok(())
    } else {
        Err(Error::PermissionDenied)
    }
}
