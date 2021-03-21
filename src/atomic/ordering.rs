use crate::sync::atomic::Ordering;
use crate::sync::atomic::Ordering::SeqCst;
use crate::sync::atomic::Ordering::AcqRel;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::Release;
use crate::sync::atomic::Ordering::Relaxed;

pub trait IsOrderingT: 'static {
    const ORDERING: Ordering;
}

pub trait IsAcquireT: IsOrderingT {}

pub trait IsReleaseT: IsOrderingT {}

pub trait IsLoadT: IsOrderingT {}

pub struct SeqCstT;

pub struct AcqRelT;

pub struct AcquireT;

pub struct ReleaseT;

pub struct RelaxedT;

impl IsOrderingT for SeqCstT { const ORDERING: Ordering = SeqCst; }

impl IsOrderingT for AcqRelT { const ORDERING: Ordering = AcqRel; }

impl IsOrderingT for AcquireT { const ORDERING: Ordering = Acquire; }

impl IsOrderingT for ReleaseT { const ORDERING: Ordering = Release; }

impl IsOrderingT for RelaxedT { const ORDERING: Ordering = Relaxed; }

impl IsAcquireT for SeqCstT {}

impl IsAcquireT for AcqRelT {}

impl IsAcquireT for AcquireT {}

impl IsReleaseT for SeqCstT {}

impl IsReleaseT for AcqRelT {}

impl IsReleaseT for ReleaseT {}

impl IsLoadT for SeqCstT {}

impl IsLoadT for AcquireT {}

impl IsLoadT for RelaxedT {}