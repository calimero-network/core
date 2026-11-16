from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_2_kind import ConsentPageCreateBodyType2Kind
from ..models.consent_page_create_body_type_2_scope_type_0 import (
    ConsentPageCreateBodyType2ScopeType0,
)
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_2_subject import (
        ConsentPageCreateBodyType2Subject,
    )
    from ..models.consent_page_create_body_type_2_scope_type_1 import (
        ConsentPageCreateBodyType2ScopeType1,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType2")


@_attrs_define
class ConsentPageCreateBodyType2:
    """
    Attributes:
        kind (ConsentPageCreateBodyType2Kind):
        scope (Union['ConsentPageCreateBodyType2ScopeType1', ConsentPageCreateBodyType2ScopeType0]):
        subject (ConsentPageCreateBodyType2Subject):
        return_to (str):
    """

    kind: ConsentPageCreateBodyType2Kind
    scope: Union[
        "ConsentPageCreateBodyType2ScopeType1", ConsentPageCreateBodyType2ScopeType0
    ]
    subject: "ConsentPageCreateBodyType2Subject"
    return_to: str

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_2_subject import (
            ConsentPageCreateBodyType2Subject,
        )
        from ..models.consent_page_create_body_type_2_scope_type_1 import (
            ConsentPageCreateBodyType2ScopeType1,
        )

        kind = self.kind.value

        scope: Union[dict[str, Any], str]
        if isinstance(self.scope, ConsentPageCreateBodyType2ScopeType0):
            scope = self.scope.value
        else:
            scope = self.scope.to_dict()

        subject = self.subject.to_dict()

        return_to = self.return_to

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "kind": kind,
                "scope": scope,
                "subject": subject,
                "returnTo": return_to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_2_subject import (
            ConsentPageCreateBodyType2Subject,
        )
        from ..models.consent_page_create_body_type_2_scope_type_1 import (
            ConsentPageCreateBodyType2ScopeType1,
        )

        d = dict(src_dict)
        kind = ConsentPageCreateBodyType2Kind(d.pop("kind"))

        def _parse_scope(
            data: object,
        ) -> Union[
            "ConsentPageCreateBodyType2ScopeType1", ConsentPageCreateBodyType2ScopeType0
        ]:
            try:
                if not isinstance(data, str):
                    raise TypeError()
                scope_type_0 = ConsentPageCreateBodyType2ScopeType0(data)

                return scope_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            scope_type_1 = ConsentPageCreateBodyType2ScopeType1.from_dict(data)

            return scope_type_1

        scope = _parse_scope(d.pop("scope"))

        subject = ConsentPageCreateBodyType2Subject.from_dict(d.pop("subject"))

        return_to = d.pop("returnTo")

        consent_page_create_body_type_2 = cls(
            kind=kind,
            scope=scope,
            subject=subject,
            return_to=return_to,
        )

        return consent_page_create_body_type_2
