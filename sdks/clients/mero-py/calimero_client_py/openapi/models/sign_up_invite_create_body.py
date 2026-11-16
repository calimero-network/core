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


T = TypeVar("T", bound="SignUpInviteCreateBody")


@_attrs_define
class SignUpInviteCreateBody:
    """
    Attributes:
        uses (Union[Unset, int]):
        ttl_secs (Union[Unset, int]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    uses: Union[Unset, int] = UNSET
    ttl_secs: Union[Unset, int] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        uses = self.uses

        ttl_secs = self.ttl_secs

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if uses is not UNSET:
            field_dict["uses"] = uses
        if ttl_secs is not UNSET:
            field_dict["ttlSecs"] = ttl_secs
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        uses = d.pop("uses", UNSET)

        ttl_secs = d.pop("ttlSecs", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        sign_up_invite_create_body = cls(
            uses=uses,
            ttl_secs=ttl_secs,
            confirm=confirm,
        )

        return sign_up_invite_create_body
