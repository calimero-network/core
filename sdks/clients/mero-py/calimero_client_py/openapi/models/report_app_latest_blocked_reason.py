from enum import Enum


class ReportAppLatestBlockedReason(str, Enum):
    AUTHORITY = "authority"
    RUNTIME = "runtime"

    def __str__(self) -> str:
        return str(self.value)
