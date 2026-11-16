from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.registration import Registration
    from ..models.confirm import Confirm


T = TypeVar("T", bound="RegisterFinishBody")


@_attrs_define
class RegisterFinishBody:
    """
    Attributes:
        challenge_id (str):
        registration (Registration): The browser's answer to `navigator.credentials.create`, each field base64url.
        invite (Union[Unset, str]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    challenge_id: str
    registration: "Registration"
    invite: Union[Unset, str] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.registration import Registration
        from ..models.confirm import Confirm

        challenge_id = self.challenge_id

        registration = self.registration.to_dict()

        invite = self.invite

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "challengeId": challenge_id,
                "registration": registration,
            }
        )
        if invite is not UNSET:
            field_dict["invite"] = invite
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.registration import Registration
        from ..models.confirm import Confirm

        d = dict(src_dict)
        challenge_id = d.pop("challengeId")

        registration = Registration.from_dict(d.pop("registration"))

        invite = d.pop("invite", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        register_finish_body = cls(
            challenge_id=challenge_id,
            registration=registration,
            invite=invite,
            confirm=confirm,
        )

        register_finish_body.additional_properties = d
        return register_finish_body

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
