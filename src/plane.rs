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

impl From<nexus::EngineStartFailure> for signal::EngineStartFailure {
    fn from(error: nexus::EngineStartFailure) -> Self {
        match error {
            nexus::EngineStartFailure::ResourceBusy(value) => Self::ResourceBusy(value),
            nexus::EngineStartFailure::ConfigurationInvalid(value) => {
                Self::ConfigurationInvalid(value)
            }
        }
    }
}

impl From<nexus::EngineStopFailure> for signal::EngineStopFailure {
    fn from(error: nexus::EngineStopFailure) -> Self {
        match error {
            nexus::EngineStopFailure::ResourceLocked(value) => Self::ResourceLocked(value),
            nexus::EngineStopFailure::ChildStillRunning(value) => Self::ChildStillRunning(value),
        }
    }
}

impl From<sema::EngineStartFailure> for nexus::EngineStartFailure {
    fn from(error: sema::EngineStartFailure) -> Self {
        match error {
            sema::EngineStartFailure::ResourceBusy(value) => Self::ResourceBusy(value),
            sema::EngineStartFailure::ConfigurationInvalid(value) => {
                Self::ConfigurationInvalid(value)
            }
        }
    }
}

impl From<sema::EngineStopFailure> for nexus::EngineStopFailure {
    fn from(error: sema::EngineStopFailure) -> Self {
        match error {
            sema::EngineStopFailure::ResourceLocked(value) => Self::ResourceLocked(value),
            sema::EngineStopFailure::ChildStillRunning(value) => Self::ChildStillRunning(value),
        }
    }
}
