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


T = TypeVar("T", bound="OrgSettingsBody")


@_attrs_define
class OrgSettingsBody:
    """The request body of `POST /orgs/{org}/settings`.

    Attributes:
        allow_guests (Union[Unset, bool]):
        allow_public_links (Union[Unset, bool]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    allow_guests: Union[Unset, bool] = UNSET
    allow_public_links: Union[Unset, bool] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        allow_guests = self.allow_guests

        allow_public_links = self.allow_public_links

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if allow_guests is not UNSET:
            field_dict["allowGuests"] = allow_guests
        if allow_public_links is not UNSET:
            field_dict["allowPublicLinks"] = allow_public_links
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        allow_guests = d.pop("allowGuests", UNSET)

        allow_public_links = d.pop("allowPublicLinks", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        org_settings_body = cls(
            allow_guests=allow_guests,
            allow_public_links=allow_public_links,
            confirm=confirm,
        )

        return org_settings_body
