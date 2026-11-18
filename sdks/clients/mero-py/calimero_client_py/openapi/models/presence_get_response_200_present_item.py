from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="PresenceGetResponse200PresentItem")


@_attrs_define
class PresenceGetResponse200PresentItem:
    """
    Attributes:
        account (str):
        device (str):
        state (Union[None, str]):
    """

    account: str
    device: str
    state: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        account = self.account

        device = self.device

        state: Union[None, str]
        state = self.state

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "account": account,
                "device": device,
                "state": state,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        account = d.pop("account")

        device = d.pop("device")

        def _parse_state(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        state = _parse_state(d.pop("state"))

        presence_get_response_200_present_item = cls(
            account=account,
            device=device,
            state=state,
        )

        presence_get_response_200_present_item.additional_properties = d
        return presence_get_response_200_present_item

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
