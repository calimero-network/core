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


T = TypeVar("T", bound="OriginsAddBody")


@_attrs_define
class OriginsAddBody:
    """
    Attributes:
        origin (str):
        rp_id (Union[Unset, str]):
        served (Union[Unset, bool]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    origin: str
    rp_id: Union[Unset, str] = UNSET
    served: Union[Unset, bool] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        origin = self.origin

        rp_id = self.rp_id

        served = self.served

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "origin": origin,
            }
        )
        if rp_id is not UNSET:
            field_dict["rpId"] = rp_id
        if served is not UNSET:
            field_dict["served"] = served
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        origin = d.pop("origin")

        rp_id = d.pop("rpId", UNSET)

        served = d.pop("served", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        origins_add_body = cls(
            origin=origin,
            rp_id=rp_id,
            served=served,
            confirm=confirm,
        )

        return origins_add_body
