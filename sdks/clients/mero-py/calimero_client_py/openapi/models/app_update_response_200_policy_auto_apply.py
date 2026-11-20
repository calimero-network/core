from enum import Enum


class AppUpdateResponse200PolicyAutoApply(str, Enum):
    NEVER = "never"
    NODESTINATIONS = "noDestinations"

    def __str__(self) -> str:
        return str(self.value)
