use game::*;

#[derive(Serialize, Deserialize, Debug)]
pub enum GridClientRequest {
    LoginAs(PlayerName),
    MoveRel(GridPoint),
    Unrecognized(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum GridClientResponse {
    LoginPrompt,
    LoggedIn(PlayerUid),
    GridUpdated(GridUpdate),
    Hangup(GridServerHangup),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum GridServerHangup {
    UnexpectedLogin,
    UnrecognizedRequest,
    InternalError
}
