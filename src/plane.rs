use crate::{
    engine::OriginRoute,
    schema::{nexus, sema},
};

impl From<OriginRoute> for nexus::OriginRoute {
    fn from(origin_route: OriginRoute) -> Self {
        Self::new(origin_route.payload())
    }
}

impl From<nexus::OriginRoute> for OriginRoute {
    fn from(origin_route: nexus::OriginRoute) -> Self {
        Self::new(origin_route.payload())
    }
}

impl From<nexus::OriginRoute> for sema::OriginRoute {
    fn from(origin_route: nexus::OriginRoute) -> Self {
        Self::new(origin_route.payload())
    }
}

impl From<OriginRoute> for sema::OriginRoute {
    fn from(origin_route: OriginRoute) -> Self {
        Self::new(origin_route.payload())
    }
}

impl From<sema::OriginRoute> for OriginRoute {
    fn from(origin_route: sema::OriginRoute) -> Self {
        Self::new(origin_route.payload())
    }
}

impl From<sema::OriginRoute> for nexus::OriginRoute {
    fn from(origin_route: sema::OriginRoute) -> Self {
        Self::new(origin_route.payload())
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
