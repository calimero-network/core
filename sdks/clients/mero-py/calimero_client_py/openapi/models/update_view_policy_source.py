from enum import Enum


class UpdateViewPolicySource(str, Enum):
    APP = "app"
    DEFAULT = "default"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
