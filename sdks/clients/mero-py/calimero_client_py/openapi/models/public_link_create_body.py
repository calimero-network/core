from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.confirm import Confirm


T = TypeVar("T", bound="PublicLinkCreateBody")


@_attrs_define
class PublicLinkCreateBody:
    """
    Attributes:
        label (Union[Unset, str]):
        expires_at (Union[Unset, int]):
        rate_per_minute (Union[Unset, int]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    label: Union[Unset, str] = UNSET
    expires_at: Union[Unset, int] = UNSET
    rate_per_minute: Union[Unset, int] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        label = self.label

        expires_at = self.expires_at

        rate_per_minute = self.rate_per_minute

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if label is not UNSET:
            field_dict["label"] = label
        if expires_at is not UNSET:
            field_dict["expiresAt"] = expires_at
        if rate_per_minute is not UNSET:
            field_dict["ratePerMinute"] = rate_per_minute
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        label = d.pop("label", UNSET)

        expires_at = d.pop("expiresAt", UNSET)

        rate_per_minute = d.pop("ratePerMinute", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        public_link_create_body = cls(
            label=label,
            expires_at=expires_at,
            rate_per_minute=rate_per_minute,
            confirm=confirm,
        )

        return public_link_create_body
