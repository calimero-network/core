from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_3_kind import ConsentPageCreateBodyType3Kind
from typing import cast

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_3_subject import (
        ConsentPageCreateBodyType3Subject,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType3")


@_attrs_define
class ConsentPageCreateBodyType3:
    """
    Attributes:
        kind (ConsentPageCreateBodyType3Kind):
        subject (ConsentPageCreateBodyType3Subject):
        return_to (str):
    """

    kind: ConsentPageCreateBodyType3Kind
    subject: "ConsentPageCreateBodyType3Subject"
    return_to: str

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_3_subject import (
            ConsentPageCreateBodyType3Subject,
        )

        kind = self.kind.value

        subject = self.subject.to_dict()

        return_to = self.return_to

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "kind": kind,
                "subject": subject,
                "returnTo": return_to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_3_subject import (
            ConsentPageCreateBodyType3Subject,
        )

        d = dict(src_dict)
        kind = ConsentPageCreateBodyType3Kind(d.pop("kind"))

        subject = ConsentPageCreateBodyType3Subject.from_dict(d.pop("subject"))

        return_to = d.pop("returnTo")

        consent_page_create_body_type_3 = cls(
            kind=kind,
            subject=subject,
            return_to=return_to,
        )

        return consent_page_create_body_type_3
