from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.receipt import Receipt
    from ..models.intents_response_200_events_item import IntentsResponse200EventsItem


T = TypeVar("T", bound="IntentsResponse200")


@_attrs_define
class IntentsResponse200:
    """
    Attributes:
        index (int):
        result (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
        events (list['IntentsResponse200EventsItem']):
        receipt (Receipt): The answering node's signature (receipt key, hex) over `(app, index, SHA-256(JCS(result)),
            node)`.
    """

    index: int
    result: Any
    events: list["IntentsResponse200EventsItem"]
    receipt: "Receipt"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.receipt import Receipt
        from ..models.intents_response_200_events_item import (
            IntentsResponse200EventsItem,
        )

        index = self.index

        result = self.result

        events = []
        for events_item_data in self.events:
            events_item = events_item_data.to_dict()
            events.append(events_item)

        receipt = self.receipt.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "index": index,
                "result": result,
                "events": events,
                "receipt": receipt,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.receipt import Receipt
        from ..models.intents_response_200_events_item import (
            IntentsResponse200EventsItem,
        )

        d = dict(src_dict)
        index = d.pop("index")

        result = d.pop("result")

        events = []
        _events = d.pop("events")
        for events_item_data in _events:
            events_item = IntentsResponse200EventsItem.from_dict(events_item_data)

            events.append(events_item)

        receipt = Receipt.from_dict(d.pop("receipt"))

        intents_response_200 = cls(
            index=index,
            result=result,
            events=events,
            receipt=receipt,
        )

        intents_response_200.additional_properties = d
        return intents_response_200

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
