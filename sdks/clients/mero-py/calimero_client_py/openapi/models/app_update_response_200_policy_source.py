from enum import Enum


class AppUpdateResponse200PolicySource(str, Enum):
    APP = "app"
    DEFAULT = "default"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
