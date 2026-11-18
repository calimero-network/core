from enum import Enum


class AppSecretsResponse200SecretsAdditionalPropertySource(str, Enum):
    APP = "app"
    CLUSTER = "cluster"

    def __str__(self) -> str:
        return str(self.value)
