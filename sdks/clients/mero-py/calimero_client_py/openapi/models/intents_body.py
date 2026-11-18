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


T = TypeVar("T", bound="IntentsBody")


@_attrs_define
class IntentsBody:
    """
    Attributes:
        method (str):
        args (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
            it is).
        idempotency_key (Union[Unset, str]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    method: str
    args: Union[Unset, Any] = UNSET
    idempotency_key: Union[Unset, str] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        method = self.method

        args = self.args

        idempotency_key = self.idempotency_key

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "method": method,
            }
        )
        if args is not UNSET:
            field_dict["args"] = args
        if idempotency_key is not UNSET:
            field_dict["idempotencyKey"] = idempotency_key
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        method = d.pop("method")

        args = d.pop("args", UNSET)

        idempotency_key = d.pop("idempotencyKey", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        intents_body = cls(
            method=method,
            args=args,
            idempotency_key=idempotency_key,
            confirm=confirm,
        )

        intents_body.additional_properties = d
        return intents_body

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
