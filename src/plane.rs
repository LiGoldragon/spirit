use crate::schema::{nexus, sema, signal};

impl From<signal::OriginRoute> for nexus::OriginRoute {
    fn from(origin_route: signal::OriginRoute) -> Self {
        Self(origin_route.0)
    }
}

impl From<nexus::OriginRoute> for signal::OriginRoute {
    fn from(origin_route: nexus::OriginRoute) -> Self {
        Self(origin_route.0)
    }
}

impl From<nexus::OriginRoute> for sema::OriginRoute {
    fn from(origin_route: nexus::OriginRoute) -> Self {
        Self(origin_route.0)
    }
}

impl From<signal::OriginRoute> for sema::OriginRoute {
    fn from(origin_route: signal::OriginRoute) -> Self {
        Self(origin_route.0)
    }
}

impl From<sema::OriginRoute> for signal::OriginRoute {
    fn from(origin_route: sema::OriginRoute) -> Self {
        Self(origin_route.0)
    }
}

impl From<sema::OriginRoute> for nexus::OriginRoute {
    fn from(origin_route: sema::OriginRoute) -> Self {
        Self(origin_route.0)
    }
}

impl From<nexus::ActorStartFailure> for signal::ActorStartFailure {
    fn from(error: nexus::ActorStartFailure) -> Self {
        match error {
            nexus::ActorStartFailure::ResourceBusy(value) => Self::ResourceBusy(value),
            nexus::ActorStartFailure::ConfigurationInvalid(value) => {
                Self::ConfigurationInvalid(value)
            }
        }
    }
}

impl From<nexus::ActorStopFailure> for signal::ActorStopFailure {
    fn from(error: nexus::ActorStopFailure) -> Self {
        match error {
            nexus::ActorStopFailure::ResourceLocked(value) => Self::ResourceLocked(value),
            nexus::ActorStopFailure::ChildStillRunning(value) => Self::ChildStillRunning(value),
        }
    }
}

impl From<sema::ActorStartFailure> for nexus::ActorStartFailure {
    fn from(error: sema::ActorStartFailure) -> Self {
        match error {
            sema::ActorStartFailure::ResourceBusy(value) => Self::ResourceBusy(value),
            sema::ActorStartFailure::ConfigurationInvalid(value) => {
                Self::ConfigurationInvalid(value)
            }
        }
    }
}

impl From<sema::ActorStopFailure> for nexus::ActorStopFailure {
    fn from(error: sema::ActorStopFailure) -> Self {
        match error {
            sema::ActorStopFailure::ResourceLocked(value) => Self::ResourceLocked(value),
            sema::ActorStopFailure::ChildStillRunning(value) => Self::ChildStillRunning(value),
        }
    }
}
