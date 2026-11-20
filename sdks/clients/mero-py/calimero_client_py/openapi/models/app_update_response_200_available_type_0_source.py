from enum import Enum


class AppUpdateResponse200AvailableType0Source(str, Enum):
    DEPLOYMENT = "deployment"
    RELEASE = "release"

    def __str__(self) -> str:
        return str(self.value)
