from enum import Enum


class AppTeamListMode(str, Enum):
    LEGACY = "legacy"
    SHARED = "shared"
    STANDALONE = "standalone"

    def __str__(self) -> str:
        return str(self.value)
