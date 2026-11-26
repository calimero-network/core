from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.receipt import Receipt
    from ..models.app_event import AppEvent


T = TypeVar("T", bound="IntentResult")


@_attrs_define
class IntentResult:
    """What a call (`/intents`) answers: the log index of its entry, the app's result, the events the caller receives, and
    the answering node's receipt.

        Attributes:
            index (int):
            result (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            events (list['AppEvent']):
            receipt (Receipt): The answering node's signature (receipt key, hex) over `(app, index, SHA-256(JCS(result)),
                node)`.
    """

    index: int
    result: Any
    events: list["AppEvent"]
    receipt: "Receipt"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.receipt import Receipt
        from ..models.app_event import AppEvent

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
        from ..models.app_event import AppEvent

        d = dict(src_dict)
        index = d.pop("index")

        result = d.pop("result")

        events = []
        _events = d.pop("events")
        for events_item_data in _events:
            events_item = AppEvent.from_dict(events_item_data)

            events.append(events_item)

        receipt = Receipt.from_dict(d.pop("receipt"))

        intent_result = cls(
            index=index,
            result=result,
            events=events,
            receipt=receipt,
        )

        intent_result.additional_properties = d
        return intent_result

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
