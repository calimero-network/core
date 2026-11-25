from enum import Enum


class OutboundAuditEntryFailure(str, Enum):
    AWAITINGAPPROVAL = "awaitingApproval"
    EGRESSOFF = "egressOff"
    FAILED = "failed"
    MALFORMED = "malformed"
    NOTAPPROVED = "notApproved"
    REFUSED = "refused"
    SECRET = "secret"

    def __str__(self) -> str:
        return str(self.value)
