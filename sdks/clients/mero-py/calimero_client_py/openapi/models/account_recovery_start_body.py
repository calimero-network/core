from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.registration import Registration


T = TypeVar("T", bound="AccountRecoveryStartBody")


@_attrs_define
class AccountRecoveryStartBody:
    """
    Attributes:
        challenge_id (str):
        code (str):
        registration (Registration): The browser's answer to `navigator.credentials.create`, each field base64url.
    """

    challenge_id: str
    code: str
    registration: "Registration"

    def to_dict(self) -> dict[str, Any]:
        from ..models.registration import Registration

        challenge_id = self.challenge_id

        code = self.code

        registration = self.registration.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "challengeId": challenge_id,
                "code": code,
                "registration": registration,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.registration import Registration

        d = dict(src_dict)
        challenge_id = d.pop("challengeId")

        code = d.pop("code")

        registration = Registration.from_dict(d.pop("registration"))

        account_recovery_start_body = cls(
            challenge_id=challenge_id,
            code=code,
            registration=registration,
        )

        return account_recovery_start_body
