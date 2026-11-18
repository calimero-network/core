from enum import Enum


class MeResponse200Via(str, Enum):
    APIKEY = "apiKey"
    SESSION = "session"

    def __str__(self) -> str:
        return str(self.value)
